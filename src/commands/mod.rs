pub mod admin;
pub mod creator_stats;
pub mod go_live;
pub mod greeter;
pub mod ping;
pub mod sticky;
pub mod tickets;
pub mod vc;
pub mod whitelabel;

use crate::{Data, Error};

pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![
        ping::ping(),
        vc::vc(),
        whitelabel::botprofile(),
        whitelabel::whitelabel(),
        sticky::sticky(),
        sticky::pin_as_sticky(),
        tickets::management::ticket(),
        admin::admin(),
        creator_stats::management::creator(),
        greeter::management::greeter(),
        go_live::management::golive(),
    ]
}
