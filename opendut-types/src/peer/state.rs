use std::net::IpAddr;
use serde::{Deserialize, Serialize};
use crate::ShortName;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PeerState {
    Down,
    Up {
        inner: PeerUpState,
        remote_host: IpAddr,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PeerUpState {
    Available,
    Blocked(PeerBlockedState),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PeerBlockedState {
    Deploying,
    Member,
    Undeploying,
}

impl Default for PeerState {
    /// PeerState Down is default State.
    /// ```
    /// use std::net::IpAddr;
    /// use std::str::FromStr;
    /// use googletest::assert_that;
    /// use opendut_types::peer::state::{PeerBlockedState, PeerState, PeerUpState};
    /// use opendut_types::ShortName;
    ///
    /// let default_state = PeerState::default();
    /// assert_eq!(default_state, PeerState::Down);
    /// ```
    fn default() -> Self {
        Self::Down
    }
}

impl ShortName for PeerState {
    /// Retrieve the short name for PeerState.
    /// ```
    /// use std::net::IpAddr;
    /// use std::str::FromStr;
    /// use googletest::assert_that;
    /// use opendut_types::peer::state::{PeerBlockedState, PeerState, PeerUpState};
    /// use opendut_types::ShortName;
    ///
    /// let down_state = PeerState::Down;
    /// assert_eq!(down_state.short_name(), "Down");
    ///
    /// let available_state = PeerState::Up { 
    ///     inner: PeerUpState::Available, 
    ///     remote_host: IpAddr::from_str("1.1.1.1").unwrap()
    /// };
    /// assert_eq!(available_state.short_name(), "Available");
    ///
    /// let deploying_state = PeerState::Up { 
    ///     inner: PeerUpState::Blocked(PeerBlockedState::Deploying), 
    ///     remote_host: IpAddr::from_str("1.1.1.1").unwrap()
    /// };
    /// assert_eq!(deploying_state.short_name(), "Deploying");
    ///
    /// let member_state = PeerState::Up { 
    ///     inner: PeerUpState::Blocked(PeerBlockedState::Member), 
    ///     remote_host: IpAddr::from_str("1.1.1.1").unwrap()
    /// };
    /// assert_eq!(member_state.short_name(), "Member");
    /// 
    /// let undeploying_state = PeerState::Up {
    ///     inner: PeerUpState::Blocked(PeerBlockedState::Undeploying), 
    ///     remote_host: IpAddr::from_str("1.1.1.1").unwrap()
    /// };
    /// assert_eq!(undeploying_state.short_name(), "Undeploying");
    /// ```
    fn short_name(&self) -> &'static str {
        match self {
            PeerState::Up { inner, .. } => match inner {
                PeerUpState::Available => "Available",
                PeerUpState::Blocked(PeerBlockedState::Deploying) => "Deploying",
                PeerUpState::Blocked(PeerBlockedState::Member) => "Member",
                PeerUpState::Blocked(PeerBlockedState::Undeploying) => "Undeploying",
            }
            PeerState::Down => "Down",
        }
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use std::str::FromStr;
    use googletest::prelude::*;
    use super::*;

    #[test]
    fn PeerState_short_name_Available() -> Result<()> {
        assert_eq!(PeerState::Up {
            inner: PeerUpState::Available,
            remote_host: IpAddr::from_str("1.1.1.1").unwrap()
        }.short_name(), "Available");
        Ok(())
    }

    #[test]
    fn PeerState_short_name_Deploying() -> Result<()> {
        assert_eq!(PeerState::Up {
            inner: PeerUpState::Blocked(PeerBlockedState::Deploying),
            remote_host: IpAddr::from_str("1.1.1.1").unwrap()
        }.short_name(), "Deploying");
        Ok(())
    }

    #[test]
    fn PeerState_short_name_Member() -> Result<()> {
        assert_eq!(PeerState::Up {
            inner: PeerUpState::Blocked(PeerBlockedState::Member),
            remote_host: IpAddr::from_str("1.1.1.1").unwrap()
        }.short_name(), "Member");
        Ok(())
    }

    #[test]
    fn PeerState_short_name_Undeploying() -> Result<()> {
        assert_eq!(PeerState::Up {
            inner: PeerUpState::Blocked(PeerBlockedState::Undeploying),
            remote_host: IpAddr::from_str("1.1.1.1").unwrap()
        }.short_name(), "Undeploying");
        Ok(())
    }

    #[test]
    fn PeerState_short_name_Down() -> Result<()> {
        assert_eq!(PeerState::Down.short_name(), "Down");
        Ok(())
    }
}
