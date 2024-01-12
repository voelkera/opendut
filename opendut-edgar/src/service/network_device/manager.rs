use std::fmt::{Debug, Formatter};
use std::io;
use std::net::Ipv4Addr;
use std::process::Command;

use anyhow::anyhow;
use futures::TryStreamExt;
use netlink_packet_route::link::nlas;
use netlink_packet_route::LinkMessage;
use regex::Regex;

use opendut_types::topology::InterfaceName;

use crate::service::network_device::gretap::Gretap;

pub struct NetworkDeviceManager {
    handle: rtnetlink::Handle,
}
impl NetworkDeviceManager {
    pub fn create() -> Result<Self> {
        let (connection, handle, _) = rtnetlink::new_connection()
            .map_err(|cause| Error::Connecting { cause })?;
        tokio::spawn(connection);

        Ok(Self { handle })
    }

    pub async fn list_interfaces(&self) -> Result<Vec<Interface>> {
        fn interface_name_from(interface: LinkMessage) -> anyhow::Result<InterfaceName> {
            let interface_name = interface.nlas.into_iter()
                .find_map(|nla| match nla {
                    nlas::Nla::IfName(name) => Some(name),
                    _ => None,
                })
                .ok_or(anyhow!("No name attribute found."))?;
            let interface_name = InterfaceName::try_from(interface_name)?;
            Ok(interface_name)
        }

        let interfaces = self.handle
            .link()
            .get()
            .execute()
            .try_collect::<Vec<_>>().await
            .map_err(|cause| Error::ListInterfaces { cause })?
            .into_iter()
            .filter_map(|interface| {
                let index = interface.header.index;
                match interface_name_from(interface) {
                    Ok(name) => Some(Interface { index, name }),
                    Err(cause) => {
                        log::warn!("Could not determine name of interface with index '{index}': {cause}");
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        Ok(interfaces)
    }

    pub async fn find_interface(&self, name: &InterfaceName) -> Result<Option<Interface>> {
        let interfaces = self.list_interfaces().await?;
        let maybe_interface = interfaces.into_iter().find(|interface| interface.name == *name);
        Ok(maybe_interface)
    }
    pub async fn try_find_interface(&self, name: &InterfaceName) -> Result<Interface> {
        self.find_interface(name).await?
            .ok_or(Error::InterfaceNotFound { name: name.clone() })
    }

    pub async fn create_empty_bridge(&self, name: &InterfaceName) -> Result<Interface> {
        self.handle
            .link()
            .add()
            .bridge(name.name())
            .execute().await
            .map_err(|cause| Error::BridgeCreation { name: name.clone(), cause })?;
        let interface = self.try_find_interface(name).await?;
        Ok(interface)
    }

    // We only support IPv4 for now, as NetBird only assigns IPv4 addresses to peers.
    // This does not prevent IPv6 traffic from being routed between peers.
    pub async fn create_gretap_v4_interface(&self, name: &InterfaceName, local_ip: &Ipv4Addr, remote_ip: &Ipv4Addr) -> Result<Interface> {
        self.handle
            .link()
            .add()
            .gretap_v4(name.name(), local_ip, remote_ip)
            .execute().await
            .map_err(|cause| Error::GretapCreation { name: name.clone(), cause })?;
        let interface = self.try_find_interface(name).await?;
        Ok(interface)
    }

    pub async fn set_interface_up(&self, interface: &Interface) -> Result<()> {
        self.handle
            .link()
            .set(interface.index)
            .up()
            .execute().await
            .map_err(|cause| Error::SetInterfaceUp { interface: interface.clone(), cause })?;
        Ok(())
    }

    pub async fn get_attributes(&self, interface: &Interface) -> Result<Vec<nlas::Nla>> {
        let interface_list = self.handle
            .link()
            .get()
            .match_index(interface.index)
            .execute()
            .try_collect::<Vec<_>>().await
            .map_err(|cause| Error::ListInterfaces { cause })?;

        let nlas = interface_list
            .first() //only one should match index
            .ok_or(Error::InterfaceNotFound { name: interface.name.clone() })?
            .nlas.clone();

        Ok(nlas)
    }

    pub async fn join_interface_to_bridge(&self, interface: &Interface, bridge: &Interface) -> Result<()> {
        self.handle
            .link()
            .set(interface.index)
            .master(bridge.index)
            .execute().await
            .map_err(|cause| Error::JoinInterfaceToBridge { interface: interface.clone(), bridge: bridge.clone(), cause })?;
        Ok(())
    }

    pub async fn delete_interface(&self, interface: &Interface) -> Result<()> {
        self.handle
            .link()
            .del(interface.index)
            .execute().await
            .map_err(|cause| Error::DeleteInterface { interface: interface.clone(), cause })?;
        Ok(())
    }

    pub async fn load_cangw_kernel_module(&self) -> Result<()> {
        Command::new("modprobe")
                    .arg("can-gw")
                    .status()
                    .map_err(|cause| Error::CanGwModprobe { cause })?;
        Ok(())
    }

    pub async fn remove_all_can_routes(&self) -> Result<()> {
        Command::new("cangw")
                    .arg("-F")
                    .status()
                    .map_err(|cause| Error::CanRouteFlushing { cause })?;
        Ok(())
    }

    pub async fn create_vcan_interface(&self, name: &InterfaceName) -> Result<Interface> {
        if self.find_interface(name).await?.is_none() {
            Command::new("ip")
                    .arg("link")
                    .arg("add")
                    .arg("dev")
                    .arg(name.name())
                    .arg("type")
                    .arg("vcan")
                    .status()
                    .map_err(|cause| Error::VCanInterfaceCreation { name: name.clone(), cause })?;
        }

        let interface = self.try_find_interface(name).await?;
        Ok(interface)
    }

    pub async fn create_can_route(&self, src: InterfaceName, dst: InterfaceName, can_fd: bool) -> Result<()> {
        self.try_find_interface(&src).await?;
        self.try_find_interface(&dst).await?;
        // TODO: Do we also have to ensure that the interface is up?

        if can_fd {
            Command::new("cangw")
                .arg("-A")
                .arg("-s")
                .arg(src.name())
                .arg("-d")
                .arg(dst.name())
                .arg("-e")
                .arg("-X")
                .status()
                .map_err(|cause| Error::CanRouteCreation { src: src.clone(), dst: dst.clone(), cause })?;
        } else {
            Command::new("cangw")
                .arg("-A")
                .arg("-s")
                .arg(src.name())
                .arg("-d")
                .arg(dst.name())
                .arg("-e")
                .status()
                .map_err(|cause| Error::CanRouteCreation { src: src.clone(), dst: dst.clone(), cause })?;
        }
        
        self.check_can_route_exists(src.clone(), dst.clone(), can_fd).await?.then(|| ()).ok_or(Error::CanRouteCreationNoCause { src: src.clone(), dst: dst.clone() })?;

        Ok(())
    }


    pub async fn check_can_route_exists(&self, src: InterfaceName, dst: InterfaceName, can_fd: bool) -> Result<bool> {
        let output = Command::new("cangw")
                .arg("-L")
                .output()
                .map_err(|cause| Error::ListCanRoutes { cause })?;

        let output_str = String::from_utf8(output.stdout).unwrap();
        
        let re = Regex::new(r"(?m)^cangw -A -s ([^\n ]+) -d ([^\n ]+) ((?:-X )?)-e #.*$").unwrap();
            
        for (_, [exist_src, exist_dst, can_fd_flag]) in re.captures_iter(&output_str).map(|c| c.extract()) {
            let exist_can_fd = can_fd_flag.trim() == "-X";
            if exist_src == src.to_string() && exist_dst == dst.to_string() && exist_can_fd == can_fd {
                return Ok(true)
            }
            
        }

        Ok(false)
    }
}

#[derive(Clone, Debug)]
pub struct Interface {
    pub index: u32,
    pub name: InterfaceName,
}
impl std::fmt::Display for Interface {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}: {}]", self.index, self.name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failure while creating bridge '{name}': {cause}")]
    BridgeCreation { name: InterfaceName, cause: rtnetlink::Error },
    #[error("Failed to establish connection to netlink: {cause}")]
    Connecting { cause: io::Error },
    #[error("Failure while deleting interface {interface}: {cause}")]
    DeleteInterface { interface: Interface, cause: rtnetlink::Error },
    #[error("Failure while creating gretap interface '{name}': {cause}")]
    GretapCreation { name: InterfaceName, cause: rtnetlink::Error },
    #[error("Interface with name '{name}' not found.")]
    InterfaceNotFound { name: InterfaceName },
    #[error("Failure while listing interfaces: {cause}")]
    ListInterfaces { cause: rtnetlink::Error },
    #[error("Failure while setting interface {interface} to state 'up': {cause}")]
    SetInterfaceUp { interface: Interface, cause: rtnetlink::Error },
    #[error("Failure while joining interface {interface} to bridge {bridge}: {cause}")]
    JoinInterfaceToBridge { interface: Interface, bridge: Interface, cause: rtnetlink::Error },
    #[error("Failure while creating virtual CAN interface '{name}': {cause}")]
    VCanInterfaceCreation { name: InterfaceName, cause: std::io::Error },
    #[error("Failure while loading can-gw kernel module: {cause}")]
    CanGwModprobe { cause: std::io::Error },
    #[error("Failure while flushing existing CAN routes: {cause}")]
    CanRouteFlushing { cause: std::io::Error },
    #[error("Failure while creating CAN route '{src}' -> '{dst}': {cause}")]
    CanRouteCreation { src: InterfaceName, dst: InterfaceName, cause: std::io::Error },
    #[error("Failure while creating CAN route '{src}' -> '{dst}'")]
    CanRouteCreationNoCause { src: InterfaceName, dst: InterfaceName},
    #[error("Failure while listing CAN routes: {cause}")]
    ListCanRoutes { cause: std::io::Error },
    #[error("{message}")]
    Other { message: String },
}
pub type Result<T> = std::result::Result<T, Error>;
