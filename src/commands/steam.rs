use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use serde_json::Value;

async fn app_details(client: &reqwest::Client, app_id: &str) -> Result<Option<Value>, Error> {
    let response: Value = client
        .get("https://store.steampowered.com/api/appdetails")
        .query(&[("appids", app_id)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let Some(item) = response.get(app_id) else {
        return Ok(None);
    };
    if !item
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    Ok(item.get("data").cloned())
}

/// Display Steam game details from a link or name.
#[poise::command(slash_command)]
pub async fn steam(
    ctx: Context<'_>,
    #[description = "Steam store URL or game name"] query: String,
    #[description = "Post the result publicly"] public: Option<bool>,
) -> Result<(), Error> {
    let public = public.unwrap_or(true);
    if public {
        ctx.defer().await?;
    } else {
        ctx.defer_ephemeral().await?;
    }

    let app_id = regex::Regex::new(r"store\.steampowered\.com/app/(\d+)")?
        .captures(&query)
        .and_then(|capture| capture.get(1).map(|id| id.as_str().to_owned()));
    let app_id = if let Some(app_id) = app_id {
        app_id
    } else {
        let response: Value = ctx
            .data()
            .http
            .get("https://store.steampowered.com/api/storesearch/")
            .query(&[("term", query.as_str()), ("l", "english"), ("cc", "US")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(id) = response
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("id"))
            .and_then(Value::as_u64)
        else {
            ctx.say(format!("Could not find a Steam game matching `{query}`."))
                .await?;
            return Ok(());
        };
        id.to_string()
    };

    let Some(details) = app_details(&ctx.data().http, &app_id).await? else {
        ctx.say("Steam did not return details for that game.")
            .await?;
        return Ok(());
    };
    let string = |key: &str, fallback: &str| {
        details
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_owned()
    };
    let name = string("name", "Unknown Game");
    let description = string("short_description", "No description available.");
    let developers = details
        .get("developers")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Unknown".into());
    let release = details
        .get("release_date")
        .and_then(|value| value.get("date"))
        .and_then(Value::as_str)
        .unwrap_or("TBA");
    let platforms = details
        .get("platforms")
        .and_then(Value::as_object)
        .map(|platforms| {
            platforms
                .iter()
                .filter(|(_, enabled)| enabled.as_bool().unwrap_or(false))
                .map(|(name, _)| name[..1].to_uppercase() + &name[1..])
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Windows".into());
    let price = if details
        .get("is_free")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "Free to Play".to_owned()
    } else {
        details
            .get("price_overview")
            .and_then(|value| value.get("final_formatted"))
            .and_then(Value::as_str)
            .unwrap_or("Unpriced / TBA")
            .to_owned()
    };
    let genres = details
        .get("genres")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("description").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "N/A".into());
    let store_url = format!("https://store.steampowered.com/app/{app_id}");
    let mut embed = serenity::CreateEmbed::new()
        .title(name)
        .url(&store_url)
        .description(description)
        .color(serenity::Colour::DARK_GREY)
        .field("Price", price, true)
        .field("Release", release, true)
        .field("Developer", developers, true)
        .field("Platforms", platforms, false)
        .field("Genres", genres, false);
    if let Some(image) = details.get("header_image").and_then(Value::as_str) {
        embed = embed.image(image);
    }
    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .ephemeral(!public),
    )
    .await?;
    Ok(())
}
