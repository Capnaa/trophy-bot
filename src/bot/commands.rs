use poise::serenity_prelude as serenity;

pub struct Data {} // User data, which is stored and accessible in all command invocations
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

/// Displays performance stats
#[poise::command(slash_command, prefix_command)]
pub async fn bench(
    ctx: Context<'_>,
) -> Result<(), Error> {
    let start = std::time::Instant::now();
    let latency = serenity::all::Timestamp::now().timestamp_millis() - ctx.created_at().timestamp_millis();
    //ctx.defer().await?;
    //tokio::task::yield_now().await;
    let time = start.elapsed().as_secs_f32();

    let locale = crate::i18n::resolve(ctx.locale());
    let response = crate::i18n::t_args(
        &locale,
        "bench-response",
        &[("latency", latency.into()), ("time", time.into())],
    );
    ctx.say(response).await?;
    log::warn!("Command `bench` dispatched in {}s", start.elapsed().as_secs_f32());
    Ok(())
}
