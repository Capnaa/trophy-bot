# Trophy Bot - Comandos, Funcionalidad y Estructura

Este documento describe todos los comandos, eventos y funcionalidades del bot, útil para implementar un bot similar desde cero.

## Comandos principales

### Gestión de trofeos

- `/create`: Crea un trofeo personalizado (nombre, descripción, emoji, valor, dedicación, imagen, detalles, firmado, intercambiable). Guarda en `guilds.data.<guild>.trophies`.
- `/edit`: Edita los datos de un trofeo existente. Modifica en `guilds.data.<guild>.trophies`.
- `/delete`: Elimina un trofeo y limpia referencias en usuarios. Borra de `guilds.data.<guild>.trophies` y actualiza `guilds.data.<guild>.users`.
- `/award`: Otorga un trofeo a un usuario, suma valor. Modifica `guilds.data.<guild>.users.<user>.trophies` y `guilds.data.<guild>.users.<user>.trophyValue`.
- `/revoke`: Revoca trofeos de un usuario, resta valor. Modifica `guilds.data.<guild>.users.<user>.trophies` y `guilds.data.<guild>.users.<user>.trophyValue`.
- `/clear`: Elimina todos los trofeos y puntuación de un usuario. Resetea `guilds.data.<guild>.users.<user>.trophies` y `guilds.data.<guild>.users.<user>.trophyValue`.

### Consulta y visualización

- `/trophies user`: Lista trofeos de un usuario, con paginación y orden. Lee de `guilds.data.<guild>.users` y `guilds.data.<guild>.trophies`.
- `/trophies guild`: Lista trofeos del servidor. Lee de `guilds.data.<guild>.trophies`.
- `/show`: Muestra detalles de un trofeo. Lee de `guilds.data.<guild>.trophies`.
- `/details`: Muestra detalles privados de un trofeo. Lee de `guilds.data.<guild>.trophies`.
- `/leaderboard`: Ranking de usuarios por puntuación. Lee de `guilds.data.<guild>.users`.
- `/panel`: Crea o elimina panel de leaderboard. Modifica `guilds.data.<guild>.panel`.

### Configuración y administración

- `/settings set`: Cambia una configuración del bot en el servidor. Modifica `guilds.data.<guild>.settings`.
- `/settings list`: Lista configuraciones actuales. Lee de `guilds.data.<guild>.settings`.
- `/rewards add/remove/clear/list`: Gestiona roles de recompensa por puntuación. Modifica `guilds.data.<guild>.rewards`.
- `/permissions add/remove/list`: Asigna o elimina permisos a roles. Modifica `guilds.data.<guild>.permissions`.
- `/imsafe`: Marca el servidor como seguro. Modifica `guilds.data.<guild>.imsafe`.
- `/forgetme`: Elimina todos los datos e imágenes del servidor y expulsa el bot. Borra `guilds.data.<guild>`.

### Utilidad e información

- `/help`: Muestra ayuda y explicación de comandos.
- `/about`: Información sobre el bot y enlaces de soporte.
- `/ping`: Muestra la latencia del bot.
- `/stats`: Estadísticas del bot (servidores, usuarios, trofeos, premios). Lee de `bot.data` y `guilds.data`.
- `/support`: Enlace al servidor de soporte.
- `/suggest`: Enlace para sugerencias.
- `/invite`: Enlace de invitación del bot.
- `/language`: Cambia el idioma del bot en el servidor. Modifica `guilds.data.<guild>.language`.

## Eventos que escucha

- `interactionCreate`: Ejecuta comandos y botones, valida permisos y roles.
- `guildCreate`: Detecta cuando el bot se une a un servidor, registra el evento y milestones.
- `guildDelete`: Detecta cuando el bot es expulsado de un servidor.
- `guildMemberAdd`: Añade rol de bienvenida en el servidor de soporte.
- `ready`: Inicializa el bot, carga comandos, idiomas y actualiza paneles.

## Estructura y datos en la base de datos

- `bot.data`: Datos globales del bot (versión, comandos, trofeos, premios, usuarios baneados).
- `guilds.data.<guild>`: Datos por servidor (trofeos, usuarios, puntuaciones, panel, configuraciones, roles de recompensa, permisos, idioma, modo seguro).
- `guilds.data.<guild>.users.<user>`: Trofeos y puntuación de cada usuario.

## Resumen de funcionalidades

- Crear, editar, eliminar y otorgar trofeos personalizados por servidor.
- Mantener base de datos de trofeos, usuarios y puntuaciones.
- Roles de recompensa automáticos por puntuación.
- Configuración flexible por servidor (idioma, formato, panel, permisos).
- Paginación y ordenamiento en listados y rankings.
- Sistema de permisos granular por rol.
- Respuestas embebidas y amigables en Discord.
- Soporte multilenguaje.
- Comandos slash (Discord.js v14).

---

Este documento cubre todas las funcionalidades y estructura de Trophy Bot para replicar su comportamiento en un nuevo bot.
