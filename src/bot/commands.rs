use poise::serenity_prelude as serenity;

pub struct Data {} // User data, which is stored and accessible in all command invocations
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

/// Displays your or another user's account creation date
#[poise::command(slash_command, prefix_command)]
pub async fn age(
    ctx: Context<'_>,
    #[description = "Selected user"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let start = std::time::Instant::now();
    let latency = serenity::all::Timestamp::now().timestamp_millis() - ctx.created_at().timestamp_millis();
    ctx.defer().await?;
    tokio::task::yield_now().await;

    let u = user.as_ref().unwrap_or_else(|| ctx.author());

    let response = format!("{}'s account was created at {}\nlatency: {latency}ms\ntime: {}s", u.name, u.created_at(), start.elapsed().as_secs_f32());
    ctx.say(response).await?;
    log::warn!("Command `age` was dispatched in {}s", start.elapsed().as_secs_f32());
    Ok(())
}
