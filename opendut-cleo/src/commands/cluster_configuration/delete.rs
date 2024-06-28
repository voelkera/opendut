use uuid::Uuid;
use opendut_carl_api::carl::{CarlClient};
use opendut_types::cluster::ClusterId;

/// Delete a cluster configuration
#[derive(clap::Parser)]
pub struct DeleteClusterConfigurationCli {
    ///ClusterID
    #[arg()]
    id: Uuid,
}

impl DeleteClusterConfigurationCli {
    pub async fn execute(self, carl: &mut CarlClient) -> crate::Result<()> {
        let id = ClusterId::from(self.id);

        let cluster_deployments = carl.cluster.list_cluster_deployments().await
            .map_err(|_| String::from("Failed to get list of cluster deployments!"))?;

        cluster_deployments.into_iter()
            .find(|cluster_deployment| cluster_deployment.id != id)
            .ok_or(format!("Cluster <{}> can not be deleted while being deployed.", id))?;

        
        let cluster_configuration = carl.cluster.delete_cluster_configuration(id).await
            .map_err(|error| format!("Failed to delete ClusterConfiguration with id <{id}>.\n  {error}"))?;

        println!("Deleted ClusterConfiguration {} <{}> successfully.", cluster_configuration.name, cluster_configuration.id);

        Ok(())
    }
}
