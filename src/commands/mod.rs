pub mod admin;
pub mod builder;
pub mod counting;
pub mod creator_stats;
pub mod custom_commands;
pub mod emotes;
pub mod fun;
pub mod go_live;
pub mod greeter;
pub mod message_logger;
pub mod moderation;
pub mod ping;
pub mod reaction_roles;
pub mod steam;
pub mod sticky;
pub mod tickets;
pub mod vc;
pub mod vents;
pub mod warnings;
pub mod whitelabel;

use crate::{Data, Error};
use poise::serenity_prelude as serenity;

pub fn guild() -> Vec<poise::Command<Data, Error>> {
    vec![
        ping::ping(),
        fun::roll(),
        fun::coinflip(),
        steam::steam(),
        message_logger::management::message_logger(),
        reaction_roles::management::reactionpanel(),
        builder::build(),
        builder::edit_message(),
        builder::edit_layout(),
        vc::vc(),
        whitelabel::botprofile(),
        whitelabel::whitelabel(),
        sticky::sticky(),
        sticky::pin_as_sticky(),
        tickets::management::ticket(),
        counting::management::counting(),
        moderation::management::moderation(),
        warnings::management::warnings(),
        warnings::management::warn(),
        admin::admin(),
        custom_commands::management::cc_admin(),
        creator_stats::management::creator(),
        greeter::management::greeter(),
        go_live::management::golive(),
        vents::vents(),
    ]
}

pub fn personal() -> Vec<poise::Command<Data, Error>> {
    let mut emote = emotes::emote();

    emote.install_context = Some(vec![
        serenity::InstallationContext::Guild,
        serenity::InstallationContext::User,
    ]);

    emote.interaction_context = Some(vec![serenity::InteractionContext::Guild]);

    vec![emote]
}

pub fn all() -> Vec<poise::Command<Data, Error>> {
    let mut commands = guild();
    commands.extend(personal());
    commands
}
