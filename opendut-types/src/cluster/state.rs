use serde::{Deserialize, Serialize};

use crate::ShortName;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClusterState {
    Undeployed,
    Deploying,
    Deployed(DeployedClusterState),
}

impl Default for ClusterState {
    fn default() -> Self {
        Self::Undeployed
    }
}

impl ShortName for ClusterState {
    fn short_name(&self) -> &'static str {
        match self {
            ClusterState::Undeployed => "Undeployed",
            ClusterState::Deploying => "Deploying",
            ClusterState::Deployed(inner) => match inner {
                DeployedClusterState::Unhealthy => "Unhealthy",
                DeployedClusterState::Healthy => "Healthy",
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DeployedClusterState {
    Unhealthy,
    Healthy,
}

impl Default for DeployedClusterState {
    fn default() -> Self {
        Self::Unhealthy
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use googletest::prelude::*;
    use super::*;

    #[test]
    fn A_ClusterState_should_have_short_name_Healthy() -> Result<()> {
        assert_eq!(ClusterState::Deployed(DeployedClusterState::Healthy).short_name(), "Healthy");
        Ok(())
    }

    #[test]
    fn A_ClusterState_should_have_short_name_Unhealthy() -> Result<()> {
        assert_eq!(ClusterState::Deployed(DeployedClusterState::Unhealthy).short_name(), "Unhealthy");
        Ok(())
    }

    #[test]
    fn A_ClusterState_should_have_short_name_Deploying() -> Result<()> {
        assert_eq!(ClusterState::Deploying.short_name(), "Deploying");
        Ok(())
    }

    #[test]
    fn A_ClusterState_should_have_short_name_Undeployed() -> Result<()> {
        assert_eq!(ClusterState::Undeployed.short_name(), "Undeployed");
        Ok(())
    }
}
