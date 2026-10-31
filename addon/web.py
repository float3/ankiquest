"""The website in a window inside Anki, signed in with the add-on's token like the phone app."""

import html
import json
import urllib.parse
from http.cookies import SimpleCookie

COOKIE = "ankiquest_session"
ROUTES = (
    "", "hour", "day", "week", "month", "year", "all", "records", "community",
    "today", "history", "settings", "friends", "privacy",
)

state = {"window": None, "account": None}


def page_url(base, path):
    return base.rstrip("/") + path


def origin(parts):
    """Use the browser's origin spelling, omitting HTTP(S) default ports."""
    host = parts.hostname or ""
    if ":" in host:
        host = "[" + host + "]"
    port = parts.port
    if port is not None and port != {"http": 80, "https": 443}.get(parts.scheme):
        host += ":" + str(port)
    return parts.scheme + "://" + host


def allowed(base, url):
    """Only ankiquest's own pages ever receive the token."""
    expected = urllib.parse.urlsplit(base.rstrip("/") + "/")
    actual = urllib.parse.urlsplit(url)
    if expected.username or expected.password or actual.username or actual.password:
        return False
    if origin(actual) != origin(expected):
        return False
    if actual.path in {expected.path + route for route in ROUTES}:
        return True
    rest = actual.path[len(expected.path):] if actual.path.startswith(expected.path) else ""
    parts = rest.split("/")
    return _conquered(parts) or _invite(parts)


def _conquered(parts):
    """A conquered card's page: /conquered/<user>/<review id>."""
    return len(parts) == 3 and parts[0] == "conquered" and parts[1] != "" and parts[2].isdigit()


def _invite(parts):
    """A friend invite: /join/<code>."""
    return len(parts) == 2 and parts[0] == "join" and parts[1] != ""


def session_script(base, user, token, language="en"):
    expected_origin = origin(urllib.parse.urlsplit(base))
    session = json.dumps({"user": user, "token": token}) if token else "null"
    return (
        "(() => { if (location.origin !== %s) return;"
        " window.ankiquestSession = %s;"
        " window.ankiquestLanguage = %s;"
        " window.dispatchEvent(new CustomEvent('ankiquest-auth')); })();"
        % (json.dumps(expected_origin), session, json.dumps(language))
    )


def session_cookie(base, token):
    """Trades the token for the site's HttpOnly session; None on servers without one."""
    from .client import ServerError, open_url

    try:
        with open_url(base.rstrip("/") + "/auth/session", b"", {"Authorization": "Bearer " + token}, "POST") as response:
            headers = response.headers.get_all("Set-Cookie") or []
    except ServerError as error:
        if error.status == 404:
            return None
        raise
    for header in headers:
        jar = SimpleCookie()
        jar.load(header)
        if COOKIE in jar:
            return jar[COOKIE].value
    raise ValueError("The ankiquest server did not return a session")


def _build(parent, base, user, token, cookie, open_link, language="en"):
    from PyQt6.QtNetwork import QNetworkCookie
    from aqt.qt import (
        QByteArray,
        QDialog,
        QUrl,
        QVBoxLayout,
        QWebEnginePage,
        QWebEngineProfile,
        QWebEngineView,
    )

    class Page(QWebEnginePage):
        def acceptNavigationRequest(self, url, kind, main_frame):
            if not main_frame or allowed(base, url.toString()):
                return True
            open_link(url.toString())
            return False

    dialog = QDialog(parent)
    dialog.setWindowTitle("ankiquest")
    dialog.resize(1000, 760)
    view = QWebEngineView(dialog)
    profile = QWebEngineProfile(dialog)
    if cookie:
        jar = QNetworkCookie(QByteArray(COOKIE.encode()), QByteArray(cookie.encode()))
        jar.setPath("/")
        jar.setHttpOnly(True)
        jar.setSecure(base.startswith("https:"))
        profile.cookieStore().setCookie(jar, QUrl(base))
    page = Page(profile, view)
    view.setPage(page)

    def signed_in(ok):
        if ok and allowed(base, view.url().toString()):
            page.runJavaScript(session_script(base, user, token, language))

    view.loadFinished.connect(signed_in)
    layout = QVBoxLayout(dialog)
    layout.setContentsMargins(0, 0, 0, 0)
    layout.addWidget(view)
    dialog.view = view
    return dialog


def open_page(mw, api, path, open_link, say):
    """Shows `path` in the ankiquest window, signing in first when the account changed."""
    language = getattr(api, "language", "en")
    account = (api.base, api.name, api.token, language)
    window = state["window"]
    if window is not None and state["account"] == account:
        _show(window, api.base, path)
        return

    def work():
        return session_cookie(api.base, api.token) if api.token else None

    def done(future):
        try:
            cookie = future.result()
        except Exception as error:
            from .language import tr
            say(tr("ankiquest: could not sign in to the website (%s)", language) % html.escape(str(error)))
            return
        if state["window"] is not None:
            state["window"].close()
        state["window"] = _build(mw, api.base, api.name, api.token, cookie, open_link, language)
        state["account"] = account
        _show(state["window"], api.base, path)

    mw.taskman.run_in_background(work, done)


def _show(window, base, path):
    from aqt.qt import QUrl

    window.view.setUrl(QUrl(page_url(base, path)))
    window.show()
    window.raise_()
    window.activateWindow()
