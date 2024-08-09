use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

use futures::future::join_all;
use futures::FutureExt;
use tracing::{debug, error, warn};

use opendut_carl_api::carl::cluster::{DeleteClusterDeploymentError, GetClusterConfigurationError, GetClusterDeploymentError, ListClusterConfigurationsError, ListClusterDeploymentsError, StoreClusterDeploymentError};
use opendut_types::cluster::{ClusterAssignment, ClusterConfiguration, ClusterDeployment, ClusterId, ClusterName, PeerClusterAssignment};
use opendut_types::peer::{PeerDescriptor, PeerId};
use opendut_types::peer::state::PeerState;
use opendut_types::topology::{DeviceDescriptor, DeviceId};
use opendut_types::util::net::NetworkInterfaceDescriptor;
use opendut_types::util::Port;

use crate::actions;
use crate::actions::{AssignClusterParams, ListPeerDescriptorsParams, UnassignClusterParams};
use crate::peer::broker::PeerMessagingBrokerRef;
use crate::persistence::error::PersistenceResult;
use crate::resources::manager::ResourcesManagerRef;
use crate::vpn::Vpn;

pub type ClusterManagerRef = Arc<Mutex<ClusterManager>>;

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum DeployClusterError {
    #[error("Cluster <{0}> not found!")]
    ClusterConfigurationNotFound(ClusterId),
    #[error("A peer for device <{device_id}> of cluster '{cluster_name}' <{cluster_id}> not found.")]
    PeerForDeviceNotFound {
        device_id: DeviceId,
        cluster_id: ClusterId,
        cluster_name: ClusterName,
    },
    #[error("Peer designated as leader <{leader_id}> of cluster '{cluster_name}' <{cluster_id}> not found.")]
    LeaderNotFound {
        leader_id: PeerId,
        cluster_id: ClusterId,
        cluster_name: ClusterName,
    },
    #[error("An error occurred while deploying cluster <{cluster_id}>:\n  {cause}")]
    Internal {
        cluster_id: ClusterId,
        cause: String
    }
}

pub struct ClusterManager {
    resources_manager: ResourcesManagerRef,
    peer_messaging_broker: PeerMessagingBrokerRef,
    vpn: Vpn,
    options: ClusterManagerOptions,
    can_server_port_counter: u16,
}

impl ClusterManager {
    pub fn new(
        resources_manager: ResourcesManagerRef,
        peer_messaging_broker: PeerMessagingBrokerRef,
        vpn: Vpn,
        options: ClusterManagerOptions,
    ) -> ClusterManagerRef {
        let can_server_port_counter = options.can_server_port_range_start;
        Arc::new(Mutex::new(Self {
            resources_manager,
            peer_messaging_broker,
            vpn,
            options,
            can_server_port_counter
        }))
    }
    #[tracing::instrument(skip(self), level="trace")]
    pub async fn deploy(&mut self, cluster_id: ClusterId) -> Result<(), DeployClusterError> {

        let cluster_config = self.resources_manager.resources(|resources| {
            resources.get::<ClusterConfiguration>(cluster_id)
        }).await
        .map_err(|cause| DeployClusterError::Internal { cluster_id, cause: cause.to_string() })?
        .ok_or(DeployClusterError::ClusterConfigurationNotFound(cluster_id))?;

        let cluster_name = cluster_config.name;

        let all_peers = actions::list_peer_descriptors(ListPeerDescriptorsParams {
            resources_manager: Arc::clone(&self.resources_manager),
        }).await.map_err(|cause| DeployClusterError::Internal { cluster_id, cause: cause.to_string() })?;


        let member_interface_mapping = determine_member_interface_mapping(cluster_config.devices, all_peers, cluster_config.leader)
            .map_err(|cause| match cause {
                DetermineMemberInterfaceMappingError::PeerForDeviceNotFound { device_id } => DeployClusterError::PeerForDeviceNotFound { device_id, cluster_id, cluster_name },
            })?;

        let member_ids = member_interface_mapping.keys().cloned().collect::<Vec<_>>();

        if let Vpn::Enabled { vpn_client } = &self.vpn {
            vpn_client.create_cluster(cluster_id, &member_ids).await
                .map_err(|cause| {
                    let message = format!("Failure while creating cluster <{cluster_id}> in VPN service.");
                    error!("{}\n  {cause}", message);
                    DeployClusterError::Internal { cluster_id, cause: message }
                })?;

            let peers_string = member_ids.iter().map(|peer| peer.to_string()).collect::<Vec<_>>().join(",");
            debug!("Created group for cluster <{cluster_id}> in VPN service, using peers: {peers_string}");
        } else {
            debug!("VPN disabled. Not creating VPN group.")
        }

        let n_peers = u16::try_from(member_interface_mapping.len())
            .map_err(|cause| DeployClusterError::Internal { cluster_id, cause: cause.to_string() })?;
        if self.options.can_server_port_range_start + n_peers >= self.options.can_server_port_range_end {
            return Err(DeployClusterError::Internal { 
                cluster_id, 
                cause: format!("Failure while creating cluster <{}>. Port range [{}, {}) specified by 'can_server_port_range_start' 
                and 'can_server_port_range_start' is too narrow for the configured number of peers ({})", 
                cluster_id, self.options.can_server_port_range_start, self.options.can_server_port_range_end, n_peers) 
            })
        } else if self.options.can_server_port_range_start + n_peers * 2 >= self.options.can_server_port_range_end {
            warn!("Port range [{}, {}) specified by 'can_server_port_range_start' 
                and 'can_server_port_range_start' is very narrow for the configured number of peers ({}). This may cause errors on EDGAR.", 
                self.options.can_server_port_range_start, self.options.can_server_port_range_end, n_peers);
        }

        // Wrap-around the counter when we reached the end of the range of usable ports
        if self.can_server_port_counter + n_peers >= self.options.can_server_port_range_end {
            self.can_server_port_counter = self.options.can_server_port_range_start;
        }
        
        let can_server_ports = (self.can_server_port_counter..self.can_server_port_counter + n_peers)
            .map(Port)
            .collect::<Vec<_>>();
        self.can_server_port_counter += n_peers;

        let member_assignments: Vec<Result<PeerClusterAssignment, DeployClusterError>> = {
            let assignment_futures = std::iter::zip(member_interface_mapping, can_server_ports)
                .map(|((peer_id, device_interfaces), can_server_port)| {
                    self.resources_manager.get::<PeerState>(peer_id)
                        .map(move |peer_state: PersistenceResult<Option<PeerState>>| {
                            let vpn_address = match peer_state {
                                Ok(peer_state) => match peer_state {
                                    Some(PeerState::Up { remote_host, .. }) => {
                                        Ok(remote_host)
                                    }
                                    Some(_) => {
                                        Err(DeployClusterError::Internal { cluster_id, cause: format!("Peer <{peer_id}> which is used in a cluster, should have a PeerState of 'Up'.") })
                                    }
                                    None => {
                                        Err(DeployClusterError::Internal { cluster_id, cause: format!("Peer <{peer_id}> which is used in a cluster, should have a PeerState associated.") })
                                    }
                                }
                                Err(cause) => {
                                    let message = format!("Error while accessing persistence to read PeerState of peer <{peer_id}>");
                                    error!("{message}:\n  {cause}");
                                    Err(DeployClusterError::Internal { cluster_id, cause: message })
                                }
                            };
                            vpn_address.map(|vpn_address|
                                PeerClusterAssignment { peer_id, vpn_address, can_server_port, device_interfaces }
                            )
                        })
                })
                .collect::<Vec<_>>();

            join_all(assignment_futures).await
        };
        let member_assignments: Vec<PeerClusterAssignment> = member_assignments.into_iter().collect::<Result<_, _>>()?;

        for member_id in member_ids {
            actions::assign_cluster(AssignClusterParams {
                resources_manager: Arc::clone(&self.resources_manager),
                peer_messaging_broker: Arc::clone(&self.peer_messaging_broker),
                peer_id: member_id,
                cluster_assignment: ClusterAssignment {
                    id: cluster_id,
                    leader: cluster_config.leader,
                    assignments: member_assignments.clone(),
                },
            }).await
            .map_err(|cause| {
                let message = format!("Failure while assigning cluster <{cluster_id}> to peer <{member_id}>.");
                error!("{}\n  {cause}", message);
                DeployClusterError::Internal { cluster_id, cause: message }
            })?;
        }

        Ok(())
    }

    #[tracing::instrument(skip(self), level="trace")]
    pub async fn get_configuration(&self, cluster_id: ClusterId) -> Result<Option<ClusterConfiguration>, GetClusterConfigurationError> {
        self.resources_manager.resources(|resources| {
            resources.get::<ClusterConfiguration>(cluster_id)
        }).await
        .map_err(|cause| GetClusterConfigurationError { cluster_id, message: cause.to_string() })
    }

    #[tracing::instrument(skip(self), level="trace")]
    pub async fn list_configuration(&self) -> Result<Vec<ClusterConfiguration>, ListClusterConfigurationsError> {
        self.resources_manager.resources(|resources| {
            resources.list::<ClusterConfiguration>()
        }).await
        .map_err(|cause| ListClusterConfigurationsError { message: cause.to_string() })
    }

    #[tracing::instrument(skip(self), level="trace")]
    pub async fn store_cluster_deployment(&mut self, deployment: ClusterDeployment) -> Result<ClusterId, StoreClusterDeploymentError> {
        let cluster_id = deployment.id;
        self.resources_manager.resources_mut(|resources| {
            let cluster_name = resources.get::<ClusterConfiguration>(deployment.id)
                .map_err(|cause| StoreClusterDeploymentError::Internal { cluster_id, cluster_name: None, cause: cause.to_string() })?
                .map(|cluster| cluster.name)
                .unwrap_or_else(|| ClusterName::try_from("unknown_cluster").unwrap());
            resources.insert(deployment.id, deployment)
                .map_err(|cause| StoreClusterDeploymentError::Internal { cluster_id, cluster_name: Some(cluster_name.clone()), cause: cause.to_string() })
        }).await?;
        if let Err(error) = self.deploy(cluster_id).await {
            error!("Failed to deploy cluster <{cluster_id}>, due to:\n  {error}");
        }
        Ok(cluster_id)
    }

    #[tracing::instrument(skip(self), level="trace")]
    pub async fn delete_cluster_deployment(&self, cluster_id: ClusterId) -> Result<ClusterDeployment, DeleteClusterDeploymentError> {

        let (deployment, configuration) = self.resources_manager
            .resources_mut(|resources| {
                resources.remove::<ClusterDeployment>(cluster_id)
                    .map_err(|cause| DeleteClusterDeploymentError::Internal { cluster_id, cluster_name: None, cause: cause.to_string() })?
                    .map(|deployment| {
                        let configuration = resources.get::<ClusterConfiguration>(cluster_id)
                            .map_err(|cause| DeleteClusterDeploymentError::Internal { cluster_id, cluster_name: None, cause: cause.to_string() })?;
                        Ok((deployment, configuration))
                    }).transpose()
            }).await?
            .ok_or(DeleteClusterDeploymentError::ClusterDeploymentNotFound { cluster_id })?;

        if let Some(configuration) = configuration {
            if let Vpn::Enabled { vpn_client } = &self.vpn {
                vpn_client.delete_cluster(cluster_id).await
                    .map_err(|error| DeleteClusterDeploymentError::Internal { cluster_id, cluster_name: Some(configuration.name.clone()), cause: error.to_string() })?;
            }

            {
                let all_peers = actions::list_peer_descriptors(ListPeerDescriptorsParams {
                    resources_manager: Arc::clone(&self.resources_manager),
                }).await.map_err(|cause| DeleteClusterDeploymentError::Internal { cluster_id, cluster_name: Some(configuration.name.clone()), cause: cause.to_string() })?;

                let member_ids = all_peers.into_iter()
                    .filter(|peer| !peer.topology.devices.iter().filter(|device| configuration.devices.contains(&device.id)).collect::<Vec<_>>().is_empty() )
                    .map(|peer| peer.id).collect::<Vec<PeerId>>();

                for member_id in member_ids {
                    actions::unassign_cluster(UnassignClusterParams {
                        resources_manager: Arc::clone(&self.resources_manager),
                        peer_id: member_id,
                    }).await
                        .map_err(|cause| {
                            let message = format!("Failure while assigning cluster <{cluster_id}> to peer <{member_id}>.");
                            error!("{}\n  {cause}", message);
                            DeleteClusterDeploymentError::Internal { cluster_id, cluster_name: Some(configuration.name.clone()), cause: message }
                        })?;
                }   
            }
            
        }

        Ok(deployment)
    }

    pub async fn get_deployment(&self, cluster_id: ClusterId) -> Result<Option<ClusterDeployment>, GetClusterDeploymentError> {
        self.resources_manager.resources(|resources| {
            resources.get::<ClusterDeployment>(cluster_id)
        }).await
        .map_err(|cause| GetClusterDeploymentError::Internal { cluster_id, cause: cause.to_string() })
    }

    #[tracing::instrument(skip(self), level="trace")]
    pub async fn list_deployment(&self) -> Result<Vec<ClusterDeployment>, ListClusterDeploymentsError> {
        self.resources_manager.resources(|resources| {
            resources.list::<ClusterDeployment>()
        }).await
        .map_err(|cause| ListClusterDeploymentsError { message: cause.to_string() })
    }
}

fn determine_member_interface_mapping(
    cluster_devices: HashSet<DeviceId>,
    all_peers: Vec<PeerDescriptor>,
    leader: PeerId,
) -> Result<HashMap<PeerId, Vec<NetworkInterfaceDescriptor>>, DetermineMemberInterfaceMappingError> {

    let mut result: HashMap<PeerId, Vec<NetworkInterfaceDescriptor>> = HashMap::new();

    result.insert(leader, Vec::new()); //will later be replaced, if leader has devices

    for device_id in cluster_devices {
        let member_interfaces = all_peers.iter().find_map(|peer| {

            let devices: Vec<DeviceDescriptor> = peer.topology.devices.iter()
                .filter(|device| device.id == device_id)
                .cloned()
                .collect();

            if devices.is_empty() {
                None
            } else {
                let interfaces = peer.network.interfaces_zipped_with_devices(&devices)
                    .into_iter()
                    .map(|(interface, _)| interface)
                    .collect::<Vec<_>>();

                Some((peer.id, interfaces))
            }
        });

        if let Some((peer, interfaces)) = member_interfaces {
            result.entry(peer)
                .or_default()
                .extend(interfaces);
        } else {
            return Err(DetermineMemberInterfaceMappingError::PeerForDeviceNotFound { device_id });
        }
    }
    Ok(result)
}

#[derive(Clone)]
pub struct ClusterManagerOptions {
    pub can_server_port_range_start: u16,
    pub can_server_port_range_end: u16,
}
impl ClusterManagerOptions {
    pub fn load(config: &config::Config) -> Result<Self, opendut_util::settings::LoadError> {
        let can_server_port_range_start = config.get::<u16>("peer.can.server_port_range_start")?;
        let can_server_port_range_end = config.get::<u16>("peer.can.server_port_range_end")?;

        Ok(ClusterManagerOptions {
            can_server_port_range_start,
            can_server_port_range_end,
        })
    }
}
#[derive(Debug, thiserror::Error)]
enum DetermineMemberInterfaceMappingError {
    #[error("Peer for device <{device_id}> not found.")]
    PeerForDeviceNotFound { device_id: DeviceId },
}


#[cfg(test)]
mod test {
    use std::collections::HashSet;
    use std::net::IpAddr;
    use std::str::FromStr;
    use std::time::Duration;

    use googletest::prelude::*;
    use rstest::{fixture, rstest};
    use tokio::sync::mpsc;

    use opendut_carl_api::proto::services::peer_messaging_broker::Downstream;
    use opendut_carl_api::proto::services::peer_messaging_broker::downstream;
    use opendut_types::cluster::ClusterName;
    use opendut_types::peer::{PeerDescriptor, PeerId, PeerLocation, PeerName, PeerNetworkDescriptor};
    use opendut_types::peer::executor::{container::{ContainerCommand, ContainerImage, ContainerName, Engine}, ExecutorKind, ExecutorDescriptors, ExecutorDescriptor};
    use opendut_types::topology::{DeviceDescription, DeviceDescriptor, DeviceId, DeviceName, Topology};
    use opendut_types::util::net::{NetworkInterfaceConfiguration, NetworkInterfaceId, NetworkInterfaceName};

    use crate::actions::{CreateClusterConfigurationParams, StorePeerDescriptorParams};
    use crate::peer::broker::{PeerMessagingBroker, PeerMessagingBrokerOptions};
    use crate::resources::manager::ResourcesManager;
    use crate::settings;

    use super::*;

    mod deploy_cluster {
        use opendut_carl_api::proto::services::peer_messaging_broker::ApplyPeerConfiguration;
        use crate::actions::StorePeerDescriptorOptions;
        use opendut_types::peer::configuration::{PeerConfiguration, PeerConfiguration2};

        use super::*;

        #[rstest]
        #[tokio::test]
        async fn deploy_cluster(
            fixture: Fixture,
            peer_a: PeerFixture,
            peer_b: PeerFixture,
        ) -> anyhow::Result<()> {

            let leader_id = peer_a.id;
            let cluster_id = ClusterId::random();
            let cluster_configuration = ClusterConfiguration {
                id: cluster_id,
                name: ClusterName::try_from("MyAwesomeCluster").unwrap(),
                leader: leader_id,
                devices: HashSet::from([peer_a.device, peer_b.device]),
            };

            let store_peer_descriptor_options = StorePeerDescriptorOptions {
                bridge_name_default: NetworkInterfaceName::try_from("br-opendut").unwrap(),
            };
            actions::store_peer_descriptor(StorePeerDescriptorParams {
                resources_manager: Arc::clone(&fixture.resources_manager),
                vpn: Vpn::Disabled,
                peer_descriptor: Clone::clone(&peer_a.descriptor),
                options: store_peer_descriptor_options.clone(),
            }).await?;

            actions::store_peer_descriptor(StorePeerDescriptorParams {
                resources_manager: Arc::clone(&fixture.resources_manager),
                vpn: Vpn::Disabled,
                peer_descriptor: Clone::clone(&peer_b.descriptor),
                options: store_peer_descriptor_options,
            }).await?;


            let mut peer_a_rx = peer_open(peer_a.id, peer_a.remote_host, Arc::clone(&fixture.peer_messaging_broker)).await?;
            let mut peer_b_rx = peer_open(peer_b.id, peer_b.remote_host, Arc::clone(&fixture.peer_messaging_broker)).await?;


            actions::create_cluster_configuration(CreateClusterConfigurationParams {
                resources_manager: Arc::clone(&fixture.resources_manager),
                cluster_configuration,
            }).await?;

            assert_that!(fixture.testee.lock().await.deploy(cluster_id).await, ok(eq(())));


            let expectation = || {
                matches_pattern!(ClusterAssignment {
                    id: eq(cluster_id),
                    leader: eq(leader_id),
                    assignments: any![
                        unordered_elements_are![
                            eq(PeerClusterAssignment {
                                peer_id: peer_a.id,
                                vpn_address: peer_a.remote_host,
                                can_server_port: Port(fixture.cluster_manager_options.can_server_port_range_start + 1),
                                device_interfaces: peer_a.interfaces.clone(),
                            }),
                            eq(PeerClusterAssignment {
                                peer_id: peer_b.id,
                                vpn_address: peer_b.remote_host,
                                can_server_port: Port(fixture.cluster_manager_options.can_server_port_range_start),
                                device_interfaces: peer_b.interfaces.clone(),
                            }),
                        ],
                        unordered_elements_are![
                            eq(PeerClusterAssignment {
                                peer_id: peer_a.id,
                                vpn_address: peer_a.remote_host,
                                can_server_port: Port(fixture.cluster_manager_options.can_server_port_range_start),
                                device_interfaces: peer_a.interfaces,
                            }),
                            eq(PeerClusterAssignment {
                                peer_id: peer_b.id,
                                vpn_address: peer_b.remote_host,
                                can_server_port: Port(fixture.cluster_manager_options.can_server_port_range_start + 1),
                                device_interfaces: peer_b.interfaces,
                            }),
                        ],
                    ]
                })
            };

            let (result, _result2) = receive_peer_configuration_message(&mut peer_a_rx).await;
            assert_that!(result.cluster_assignment.unwrap(), Clone::clone(&expectation)());

            let (result, _result2) = receive_peer_configuration_message(&mut peer_b_rx).await;
            assert_that!(result.cluster_assignment.unwrap(), expectation());

            Ok(())
        }

        async fn peer_open(peer_id: PeerId, peer_remote_host: IpAddr, peer_messaging_broker: PeerMessagingBrokerRef) -> anyhow::Result<mpsc::Receiver<Downstream>> {
            let (_peer_tx, mut peer_rx) = peer_messaging_broker.open(peer_id, peer_remote_host).await?;
            receive_peer_configuration_message(&mut peer_rx).await; //initial peer configuration after connect
            Ok(peer_rx)
        }

        async fn receive_peer_configuration_message(peer_rx: &mut mpsc::Receiver<Downstream>) -> (PeerConfiguration, PeerConfiguration2) {
            let message = tokio::time::timeout(Duration::from_millis(500), peer_rx.recv()).await
                .unwrap().unwrap().message.unwrap();

            if let downstream::Message::ApplyPeerConfiguration(ApplyPeerConfiguration {
                configuration: Some(peer_configuration),
                configuration2: Some(peer_configuration2),
            }) = message {
                (
                    peer_configuration.try_into().unwrap(),
                    peer_configuration2.try_into().unwrap()
                )
            } else {
                panic!("Did not receive valid message. Received this instead: {message:?}")
            }
        }
    }

    #[rstest]
    #[tokio::test]
    async fn deploy_should_fail_for_unknown_cluster(fixture: Fixture) -> anyhow::Result<()> {
        let unknown_cluster = ClusterId::random();

        assert_that!(
            fixture.testee.lock().await.deploy(unknown_cluster).await,
            err(eq(DeployClusterError::ClusterConfigurationNotFound(unknown_cluster)))
        );

        Ok(())
    }

    #[rstest]
    fn should_determine_member_interface_mapping() -> anyhow::Result<()> {

        fn device_and_interface(id: DeviceId, interface_name: NetworkInterfaceName) -> (DeviceDescriptor, NetworkInterfaceDescriptor) {
            let network_interface = NetworkInterfaceDescriptor {
                id: NetworkInterfaceId::random(),
                name: interface_name,
                configuration: NetworkInterfaceConfiguration::Ethernet,
            };
            let device = DeviceDescriptor {
                id,
                name: DeviceName::try_from(format!("test-device-{id}")).unwrap(),
                description: None,
                interface: network_interface.id,
                tags: Vec::new(),
            };
            (device, network_interface)
        }

        let (device_a, interface_a) = device_and_interface(DeviceId::random(), NetworkInterfaceName::try_from("a")?);
        let (device_b, interface_b) = device_and_interface(DeviceId::random(), NetworkInterfaceName::try_from("b")?);
        let (device_c, interface_c) = device_and_interface(DeviceId::random(), NetworkInterfaceName::try_from("c")?);

        let cluster_devices = HashSet::from([device_a.id, device_b.id, device_c.id]);

        fn peer_descriptor(id: PeerId, devices: Vec<DeviceDescriptor>, interfaces: Vec<NetworkInterfaceDescriptor>) -> PeerDescriptor {
            PeerDescriptor {
                id,
                name: PeerName::try_from(format!("peer-{id}")).unwrap(),
                location: PeerLocation::try_from("Ulm").ok(),
                network: PeerNetworkDescriptor {
                    interfaces,
                    bridge_name: Some(NetworkInterfaceName::try_from("br-custom").unwrap()),
                },
                topology: Topology {
                    devices,
                },
                executors: ExecutorDescriptors { executors: vec![] },
            }
        }

        let peer_1 = peer_descriptor(PeerId::random(), vec![device_a.clone()], vec![interface_a.clone()]);
        let peer_2 = peer_descriptor(PeerId::random(), vec![device_b.clone(), device_c.clone()], vec![interface_b.clone(), interface_c.clone()]);
        let peer_leader = peer_descriptor(PeerId::random(), vec![], vec![]);


        let all_peers = vec![peer_1.clone(), peer_2.clone(), peer_leader.clone()];
        let leader = peer_leader.id;

        let result = determine_member_interface_mapping(cluster_devices, all_peers, leader)?;

        assert_that!(
            result,
            unordered_elements_are![
                (eq(peer_1.id), unordered_elements_are![eq(interface_a)]),
                (eq(peer_2.id), unordered_elements_are![eq(interface_b), eq(interface_c)]),
                (eq(peer_leader.id), empty()),
            ]
        );
        Ok(())
    }

    struct Fixture {
        testee: ClusterManagerRef,
        resources_manager: ResourcesManagerRef,
        peer_messaging_broker: PeerMessagingBrokerRef,
        cluster_manager_options: ClusterManagerOptions,
    }
    #[fixture]
    fn fixture() -> Fixture {
        let settings = settings::load_defaults().unwrap();

        let resources_manager = ResourcesManager::new_in_memory();
        let peer_messaging_broker = PeerMessagingBroker::new(
            Arc::clone(&resources_manager),
            PeerMessagingBrokerOptions::load(&settings.config).unwrap(),
        );

        let cluster_manager_options = ClusterManagerOptions::load(&settings.config).unwrap();

        let testee = ClusterManager::new(
            Arc::clone(&resources_manager),
            Arc::clone(&peer_messaging_broker),
            Vpn::Disabled,
            cluster_manager_options.clone(),
        );
        Fixture {
            testee,
            resources_manager,
            peer_messaging_broker,
            cluster_manager_options,
        }
    }

    #[fixture]
    fn peer_a() -> PeerFixture {
        peer_fixture("PeerA")
    }
    #[fixture]
    fn peer_b() -> PeerFixture {
        peer_fixture("PeerB")
    }

    struct PeerFixture {
        id: PeerId,
        device: DeviceId,
        interfaces: Vec<NetworkInterfaceDescriptor>,
        remote_host: IpAddr,
        descriptor: PeerDescriptor,
    }
    fn peer_fixture(peer_name: &str) -> PeerFixture {
        let device = DeviceId::random();

        let id = PeerId::random();
        let remote_host = IpAddr::from_str("1.1.1.1").unwrap();
        let network_interface_id = NetworkInterfaceId::random();
        let interfaces = vec![
            NetworkInterfaceDescriptor {
                id: network_interface_id,
                name: NetworkInterfaceName::try_from("eth0").unwrap(),
                configuration: NetworkInterfaceConfiguration::Ethernet,
            }
        ];

        let descriptor = PeerDescriptor {
            id,
            name: PeerName::try_from(peer_name).unwrap(),
            location: PeerLocation::try_from("Ulm").ok(),
            network: PeerNetworkDescriptor {
                interfaces: interfaces.clone(),
                bridge_name: Some(NetworkInterfaceName::try_from("br-opendut-1").unwrap()),
            },
            topology: Topology {
                devices: vec![
                    DeviceDescriptor {
                        id: device,
                        name: DeviceName::try_from(format!("{peer_name}_Device_1")).unwrap(),
                        description: DeviceDescription::try_from("Huii").ok(),
                        interface: network_interface_id,
                        tags: vec![],
                    }
                ]
            },
            executors: ExecutorDescriptors {
                executors: vec![
                    ExecutorDescriptor{
                        kind: ExecutorKind::Container {
                            engine: Engine::Docker,
                            name: ContainerName::Empty,
                            image: ContainerImage::try_from("testUrl").unwrap(),
                            volumes: vec![],
                            devices: vec![],
                            envs: vec![],
                            ports: vec![],
                            command: ContainerCommand::Default,
                            args: vec![],
                        },
                        results_url: None,
                    }
                ],
            },
        };
        PeerFixture {
            id,
            device,
            interfaces,
            remote_host,
            descriptor
        }
    }
}
