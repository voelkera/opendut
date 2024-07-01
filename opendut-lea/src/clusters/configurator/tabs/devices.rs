use leptos::{component, IntoView, MaybeSignal, RwSignal, view};

use crate::clusters::configurator::DeviceSelector;
use crate::clusters::configurator::types::UserClusterConfiguration;
use crate::clusters::overview::IsDeployed;

#[component]
pub fn DevicesTab(cluster_configuration: RwSignal<UserClusterConfiguration>, deployed_signal: MaybeSignal<IsDeployed>) -> impl IntoView {

    view! {
        <div>
            <DeviceSelector cluster_configuration=cluster_configuration deployed_signal/>
        </div>
    }
}
