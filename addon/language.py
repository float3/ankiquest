"""Translate authored desktop labels before formatting names or messages."""

SPANISH = {
    "Cancel": "Cancelar", "Save": "Guardar", "Server": "Servidor", "Player": "Jugador", "Token": "Token",
    "Leaderboard": "Clasificación", "Friends": "Amigos", "Decks": "Mazos", "Notify": "Avisar",
    "Test connection": "Probar conexión", "Upload everything again": "Volver a subir todo",
    "Tell me when my place on the leaderboard changes": "Avísame cuando cambie mi puesto en la clasificación",
    "Warn me before my streak ends": "Avísame antes de que termine mi racha",
    "Deck completion notifications": "Avisos de mazos completados",
    "The people you pick hear once a day when you finish a shared deck.": "Se avisa una vez al día a las personas elegidas cuando completas un mazo compartido.",
    "Nobody else plays yet.": "Todavía no hay otros jugadores.", "All / none": "Todos / ninguno",
    "Nudge me when a place, my best day or the next level is within reach": "Dame un toque cuando pueda alcanzar un puesto, mi mejor día o el siguiente nivel",
    "#%d this week": "Puesto %d esta semana", "Lv %d  %s/%s XP": "Nv. %d  %s/%s XP",
    "🔥 %d day streak": "🔥 Racha de %d días", "streak at risk today": "racha en riesgo hoy", "📬 %d new": "📬 %d nuevos",
    "Set the server in ankiquest settings first.": "Configura primero el servidor en los ajustes de ankiquest.",
    "ankiquest settings saved.": "Ajustes de ankiquest guardados.",
    "Fill in the server, player and token first.": "Rellena primero el servidor, el jugador y el token.",
    "Connected as %s, level %d.": "Conectado como %s, nivel %d.",
    "Uploading your whole review history…": "Subiendo todo tu historial de repasos…",
    "Set the server, player and token in ankiquest settings first.": "Configura primero el servidor, el jugador y el token en los ajustes de ankiquest.",
    "ankiquest: could not load your decks (%s)": "ankiquest: no se pudieron cargar tus mazos (%s)",
    "Pick at least one person to notify, or share no decks.": "Elige al menos una persona a la que avisar o desactiva el envío de mazos.",
    "ankiquest: could not save your decks (%s)": "ankiquest: no se pudieron guardar tus mazos (%s)",
    "%d deck%s shared.": "Se comparten %d mazo%s.",
    "%d deck shared.": "Se comparte %d mazo.", "%d decks shared.": "Se comparten %d mazos.",
    "ankiquest settings…": "Ajustes de ankiquest…", "ankiquest deck notifications…": "Avisos de mazos de ankiquest…",
    "ankiquest inbox…": "Bandeja de entrada de ankiquest…", "ankiquest on the web…": "ankiquest en la web…",
    "ankiquest: could not sign in to the website (%s)": "ankiquest: no se pudo iniciar sesión en la web (%s)",
}


def tr(source, language=None):
    if language is None:
        from anki.lang import current_lang
        language = current_lang
    return SPANISH.get(source, source) if language.lower().replace("_", "-").split("-")[0] == "es" else source
