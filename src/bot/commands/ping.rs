//! `/ping` — real gateway ping + measured round trip (F35).
//!
//! Spec: docs/specs/commands-utility.md § /ping. The legacy bot reported the
//! interaction→defer time as "Bot Latency"; here we report the shard's
//! gateway heartbeat latency plus a freshly measured reply round trip.

use std::time::{Duration, Instant};

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// Current bot ping! If the bot doesn't answer then ping is probably over 5000ms and very likely down
#[poise::command(slash_command)]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);

    // Shard gateway heartbeat latency (zero right after connecting).
    let gateway = ctx.ping().await;

    // Measured round trip: time to create the initial response.
    let start = Instant::now();
    let reply = ctx
        .send(
            poise::CreateReply::default().embed(
                serenity::CreateEmbed::new()
                    .title(i18n::t(&locale, "ping-title"))
                    .description(i18n::t(&locale, "ping-measuring"))
                    .colour(util::COLOR_MAIN),
            ),
        )
        .await?;
    let round_trip = start.elapsed();

    let gateway_value = match gateway_latency_ms(gateway) {
        Some(ms) => i18n::t_args(&locale, "ping-latency-value", &[("ms", ms.into())]),
        None => i18n::t(&locale, "ping-gateway-unknown"),
    };
    let round_trip_value = i18n::t_args(
        &locale,
        "ping-latency-value",
        &[("ms", latency_ms(round_trip).into())],
    );

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "ping-title"))
        .colour(util::COLOR_MAIN)
        .field(i18n::t(&locale, "ping-gateway-label"), gateway_value, true)
        .field(
            i18n::t(&locale, "ping-round-trip-label"),
            round_trip_value,
            true,
        );

    reply
        .edit(ctx, poise::CreateReply::default().embed(embed))
        .await?;
    Ok(())
}

/// Converts a measured latency to whole milliseconds (saturating).
pub fn latency_ms(latency: Duration) -> u64 {
    u64::try_from(latency.as_millis()).unwrap_or(u64::MAX)
}

/// Gateway latency for display. Serenity reports `Duration::ZERO` until the
/// first heartbeat is acknowledged, which is "not measured yet", not "0 ms".
pub fn gateway_latency_ms(latency: Duration) -> Option<u64> {
    if latency.is_zero() {
        None
    } else {
        Some(latency_ms(latency))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_ms_truncates_to_whole_milliseconds() {
        assert_eq!(latency_ms(Duration::from_micros(1500)), 1);
        assert_eq!(latency_ms(Duration::from_millis(42)), 42);
        assert_eq!(latency_ms(Duration::ZERO), 0);
    }

    #[test]
    fn gateway_latency_zero_means_not_measured() {
        assert_eq!(gateway_latency_ms(Duration::ZERO), None);
        assert_eq!(gateway_latency_ms(Duration::from_millis(87)), Some(87));
    }

    #[test]
    fn ping_messages_exist_in_catalog() {
        let locale = i18n::resolve(None);
        let value = i18n::t_args(&locale, "ping-latency-value", &[("ms", 42.into())]);
        assert!(value.contains("42"), "got: {value}");
        assert_ne!(i18n::t(&locale, "ping-title"), "ping-title");
        assert_ne!(i18n::t(&locale, "ping-measuring"), "ping-measuring");
        assert_ne!(i18n::t(&locale, "ping-gateway-label"), "ping-gateway-label");
        assert_ne!(i18n::t(&locale, "ping-round-trip-label"), "ping-round-trip-label");
        assert_ne!(i18n::t(&locale, "ping-gateway-unknown"), "ping-gateway-unknown");
    }
}
