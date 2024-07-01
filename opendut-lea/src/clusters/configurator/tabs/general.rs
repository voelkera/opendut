use leptos::{component, IntoView, MaybeSignal, RwSignal, SignalGet, view};

use crate::clusters::configurator::components::ClusterNameInput;
use crate::clusters::configurator::types::UserClusterConfiguration;
use crate::clusters::overview::IsDeployed;
use crate::components::ReadOnlyInput;

#[component]
pub fn GeneralTab(cluster_configuration: RwSignal<UserClusterConfiguration>, deployed_signal: MaybeSignal<IsDeployed>) -> impl IntoView {

    let cluster_id = MaybeSignal::derive(move || cluster_configuration.get().id.to_string());

    view! {
        <div>
            <ReadOnlyInput
                label="Cluster ID"
                value=cluster_id
            />
            <ClusterNameInput
                cluster_configuration=cluster_configuration
                deployed_signal=deployed_signal
            />
        </div>
    }
}
