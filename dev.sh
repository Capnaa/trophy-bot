#!/bin/sh
# Docker helper for the Rust bot. Actions: build / run / up / down / logs / clean.

mkdir -p data images
touch .env

action="$1"

case "$action" in
  build)
    docker compose build
    ;;
  clean)
    docker compose down --rmi all
    ;;
  run)
    docker compose up
    ;;
  up)
    docker compose up -d
    ;;
  down)
    docker compose down
    ;;
  logs)
    docker compose logs -f
    ;;
  migrate)
    docker compose run --rm trophybot up
    ;;
  import)
    docker compose run --rm trophybot import --legacy-db /app/json.sqlite
    ;;
  *)
    docker compose down --rmi all
    docker compose build
    docker compose up
    docker compose down --rmi all
    ;;
esac
