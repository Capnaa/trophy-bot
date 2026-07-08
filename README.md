# Trophy Bot

<div align="center">
  <a href = "https://github.com/AntikoreDev/trophy-bot/pulls"><img alt = "Pull Requests" src = "https://img.shields.io/github/issues-pr/AntikoreDev/trophy-bot?style=for-the-badge"></a>
  <a href = "https://github.com/AntikoreDev/trophy-bot/issues"><img alt = "Issues" src = "https://img.shields.io/github/issues/AntikoreDev/trophy-bot?style=for-the-badge"></a>
  <a href = "https://github.com/AntikoreDev/trophy-bot/graphs/contributors"><img alt = "Contributors" src = "https://img.shields.io/github/contributors/AntikoreDev/trophy-bot?style=for-the-badge"></a>
  <a href = "https://github.com/AntikoreDev/trophy-bot/stargazers"><img alt = "Stars" src = "https://img.shields.io/github/stars/AntikoreDev/trophy-bot?style=for-the-badge"></a>
  <br>
  <a href = "https://github.com/AntikoreDev/trophy-bot" onClick = "return false"><img alt = "Repo size" src = "https://img.shields.io/github/repo-size/AntikoreDev/trophy-bot?style=for-the-badge"></a>
  <a href = "https://github.com/AntikoreDev/trophy-bot/blob/main/LICENSE"><img alt = "License" src = "https://img.shields.io/github/license/AntikoreDev/trophy-bot?style=for-the-badge"></a>
</div>

With **Trophy Bot**, you can award trophies to users. You can create your own trophies per server with a custom name, description, image and even value, and assign them to users by doing stuff you think deserves to be awarded.

You can then give reward roles for reaching certain scores, or having a leaderboard for the user scores.

<a href="https://discord.com/oauth2/authorize?client_id=985134052665356299&permissions=34816&scope=applications.commands%20bot">Add Trophy Bot to your server</a>

To get started, simply type `/help` in your server to see a few basic info.

## Features

* Create and customize trophies for your server (name, description, emoji, image, value).
* Award trophies to users — with autocomplete, bulk awards, and a full audit of who awarded what.
* Server leaderboard and auto-updating leaderboard panels.
* Role rewards assigned automatically when users reach score thresholds.
* Per-server settings (dedication display, role stacking, leaderboard format, and more).
* Localized replies based on each user's Discord language (English shipped; more via Fluent catalogs).

## Tech

Rewritten in **Rust** (Serenity + Poise + SeaORM) with a normalized database — SQLite for local development, PostgreSQL for production, selected by `DATABASE_URL`. The previous Node.js implementation was fully migrated: same commands and features, with its known bugs fixed. See [docs/](docs/README.md) for specs, architecture decisions and the migration plan.

## Running

```bash
cp .env.example .env          # set DISCORD_TOKEN, DISCORD_BOT_ID, DATABASE_URL
cargo run -- up               # apply database migrations
cargo run                     # start the bot
```

Other CLI subcommands: `status` / `down` / `fresh` / `refresh` / `reset` (schema migrations), `import --legacy-db <file>` (one-shot legacy quick.db data migration), `smoke` (end-to-end flow against the test guild).

### Docker

```bash
./dev.sh build && ./dev.sh up   # or: docker compose up -d
./dev.sh migrate                # apply migrations inside the container
./dev.sh logs
```

The bot shuts down gracefully on `docker stop` (SIGTERM).

## Development

* `cargo test` — full suite (no external services or production data needed).
* `cargo test -- --ignored` — extra validations against a production data snapshot, if present.
* Documentation index: [docs/README.md](docs/README.md). Contributions follow [CONTRIBUTING.md](CONTRIBUTING.md).

## Support Server

You can [join our support server](https://discord.gg/kNmgU44xgU) to get help and report issues.

## Contributing

If you like this bot, you can help by contributing to it.

Or, [buy a coffee for Antikore](https://ko-fi.com/antikore), the creator of this bot.

## About change of maintainer to jhg

It'll continue development of new features and improvements, and maintenance for fix bugs,
 without major changes for users. The original developer needs to focus on other projects,
 and that's why this maintenance change is happening. Nothing to worry about.

* What about the [terms of service](./TERMS.md)?
  * It'll continue to be same as the original bot for [Jesus' hosted version](https://github.com/jhg).
  * Self-hosted versions or forks are exempt from these terms.

* What about the [privacy policy](./PRIVACY.md)?
  * Will continue to be same as the original bot for [Jesus' hosted version](https://github.com/jhg).
  * Self-hosted versions or forks are exempt from these terms.
