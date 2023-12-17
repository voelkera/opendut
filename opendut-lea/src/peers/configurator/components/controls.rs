use leptos::*;
use opendut_types::peer::{PeerDescriptor, PeerId};
use crate::app::{ExpectGlobals, use_app_globals};

use crate::components::{ButtonColor, ButtonSize, ButtonState, ButtonStateSignalProvider, ConfirmationButton, FontAwesomeIcon, IconButton};
use crate::peers::configurator::types::UserPeerConfiguration;
use crate::routing::{navigate_to, WellKnownRoutes};

#[component]
pub fn Controls(
    configuration: ReadSignal<UserPeerConfiguration>,
    is_valid_peer_configuration: Signal<bool>,
) -> impl IntoView {

    view! {
        <div class="buttons">
            <SavePeerButton configuration is_valid_peer_configuration />
            <DeletePeerButton configuration />
        </div>
    }
}

#[component]
fn SavePeerButton(
    configuration: ReadSignal<UserPeerConfiguration>,
    is_valid_peer_configuration: Signal<bool>,
) -> impl IntoView {

    let globals = use_app_globals();

    let store_action = create_action(move |_: &()| {
        async move {
            let mut carl = globals.expect_client();
            let peer_descriptor = PeerDescriptor::try_from(configuration.get_untracked());
            match peer_descriptor {
                Ok(peer_descriptor) => {
                    let peer_id = peer_descriptor.id;
                    let result = carl.peers.store_peer_descriptor(peer_descriptor).await;
                    match result {
                        Ok(_) => {
                            log::info!("Successfully stored peer: {peer_id}");
                        }
                        Err(cause) => {
                            log::error!("Failed to create peer <{peer_id}>, due to error: {cause:?}");
                        }
                    }
                }
                Err(error) => {
                    log::error!("Failed to dispatch create peer action, due to misconfiguration!\n  {error}");
                }
            }
        }
    });

    let button_state = MaybeSignal::derive(move || {
        if store_action.pending().get() {
            ButtonState::Loading
        }
        else {
            if is_valid_peer_configuration.get() {
                ButtonState::Enabled
            }
            else {
                ButtonState::Disabled
            }
        }
    });

    view! {
        <IconButton
            icon=FontAwesomeIcon::Save
            color=ButtonColor::Info
            size=ButtonSize::Normal
            state=button_state
            label="Save Peer"
            on_action=move || {
                store_action.dispatch(());
            }
        />
    }
}

#[component]
fn DeletePeerButton(configuration: ReadSignal<UserPeerConfiguration>) -> impl IntoView {

    let globals = use_app_globals();

    let delete_action = create_action(move |_: &PeerId| {
        async move {
            let mut carl = globals.expect_client();
            let peer_id = configuration.get_untracked().id;
            let result = carl.peers.delete_peer_descriptor(peer_id).await;
            match result {
                Ok(_) => {
                    log::info!("Successfully deleted peer: {}", peer_id);
                    navigate_to(WellKnownRoutes::PeersOverview);
                }
                Err(cause) => {
                    log::error!("Failed to delete peer <{peer_id}>, due to error: {cause:?}");
                }
            }
        }
    });

    let button_state = delete_action
        .pending()
        .derive_loading();

    view! {
        <ConfirmationButton
            icon=FontAwesomeIcon::TrashCan
            color=ButtonColor::Danger
            size=ButtonSize::Normal
            state=button_state
            label="Remove Peer?"
            on_conform=move || {
                configuration.with_untracked(|config| {
                    delete_action.dispatch(config.id);
                });
            }
        />
    }
}
