use plotters::prelude::*;
use sqlx::{Row, SqlitePool};
use std::error::Error;

type ChartError = Box<dyn Error + Send + Sync>;

struct DailyMetrics {
    day: String,
    twitch_followers: i64,
    yt_subs: i64,
    discord_members: i64,
}

async fn fetch_daily_metrics(
    pool: &SqlitePool,
    creator_id: i64,
) -> Result<Vec<DailyMetrics>, ChartError> {
    let rows = sqlx::query(
        r#"
        SELECT
            date(timestamp) AS day,
            MAX(twitch_followers) AS twitch_followers,
            MAX(yt_subs) AS yt_subs,
            MAX(discord_members) AS discord_members
        FROM metrics
        WHERE creator_id = ?
          AND timestamp >= strftime('%Y-%m-%dT%H:%M:%S', 'now', '-29 days')
        GROUP BY date(timestamp)
        ORDER BY day ASC
        "#,
    )
    .bind(creator_id)
    .fetch_all(pool)
    .await?;

    let metrics = rows
        .into_iter()
        .map(|row| DailyMetrics {
            day: row.get("day"),
            twitch_followers: row.get::<Option<i64>, _>("twitch_followers").unwrap_or(0),
            yt_subs: row.get::<Option<i64>, _>("yt_subs").unwrap_or(0),
            discord_members: row.get::<Option<i64>, _>("discord_members").unwrap_or(0),
        })
        .collect();

    Ok(metrics)
}

pub async fn generate_growth_chart(
    pool: &SqlitePool,
    creator_id: i64,
    output_path: &str,
) -> Result<(), ChartError> {
    let metrics = fetch_daily_metrics(pool, creator_id).await?;

    if metrics.is_empty() {
        return Err("No historical metrics available for this creator".into());
    }

    let root = BitMapBackend::new(output_path, (1200, 900)).into_drawing_area();

    root.fill(&RGBColor(24, 25, 35))?;

    let areas = root.split_evenly((3, 1));

    let charts = [
        (
            "Twitch Followers",
            metrics
                .iter()
                .map(|m| m.twitch_followers)
                .collect::<Vec<_>>(),
            RGBColor(145, 70, 255),
        ),
        (
            "YouTube Subscribers",
            metrics.iter().map(|m| m.yt_subs).collect::<Vec<_>>(),
            RGBColor(231, 76, 60),
        ),
        (
            "Discord Members",
            metrics
                .iter()
                .map(|m| m.discord_members)
                .collect::<Vec<_>>(),
            RGBColor(46, 204, 113),
        ),
    ];

    for (area, (title, values, color)) in areas.into_iter().zip(charts.iter()) {
        let max_value = values.iter().copied().max().unwrap_or(0);
        let min_value = values.iter().copied().min().unwrap_or(0);

        let padding = ((max_value - min_value) / 10).max(5);
        let y_min = (min_value - padding).max(0);
        let y_max = max_value + padding;

        let mut chart = ChartBuilder::on(&area)
            .caption(*title, ("sans-serif", 30).into_font().color(&WHITE))
            .margin(20)
            .x_label_area_size(40)
            .y_label_area_size(75)
            .build_cartesian_2d(0usize..metrics.len().max(2), y_min..y_max.max(y_min + 1))?;

        chart
            .configure_mesh()
            .axis_style(RGBColor(100, 100, 115))
            .bold_line_style(RGBColor(65, 65, 80))
            .light_line_style(RGBColor(45, 45, 60))
            .label_style(("sans-serif", 16).into_font().color(&WHITE))
            .x_labels(6)
            .y_labels(5)
            .x_label_formatter(&|index| {
                metrics
                    .get(*index)
                    .map(|m| m.day.get(5..).unwrap_or(&m.day).to_string())
                    .unwrap_or_default()
            })
            .draw()?;

        chart.draw_series(LineSeries::new(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| (index, *value)),
            color.stroke_width(4),
        ))?;

        chart.draw_series(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| Circle::new((index, *value), 5, color.filled())),
        )?;
    }

    root.present()?;

    Ok(())
}
