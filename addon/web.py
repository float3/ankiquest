"""The website in a window inside Anki, signed in with the add-on's token like the phone app."""

import json
import urllib.error
import urllib.parse
import urllib.request
from http.cookies import SimpleCookie

COOKIE = "ankiquest_session"
ROUTES = ("", "hour", "day", "week", "month", "year", "all", "records", "community")

state = {"window": None, "account": None}


def page_url(base, path):
    return base.rstrip("/") + path


def allowed(base, url):
    """Only ankiquest's own pages ever receive the token."""
    expected = urllib.parse.urlsplit(base.rstrip("/") + "/")
    actual = urllib.parse.urlsplit(url)
    if expected.username or expected.password or actual.username or actual.password:
        return False
    if (actual.scheme, actual.hostname, actual.port) != (
        expected.scheme,
        expected.hostname,
        expected.port,
    ):
        return False
    return actual.path in {expected.path + route for route in ROUTES}


def session_script(base, user, token, language="en"):
    origin = "%s://%s" % urllib.parse.urlsplit(base)[:2]
    session = json.dumps({"user": user, "token": token}) if token else "null"
    return (
        "(() => { if (location.origin !== %s) return;"
        " window.ankiquestSession = %s;"
        " window.ankiquestLanguage = %s;"
        " window.dispatchEvent(new CustomEvent('ankiquest-auth')); })();"
        % (json.dumps(origin), session, json.dumps(language))
    )


def session_cookie(base, token):
    """Trades the token for the site's HttpOnly session; None on servers without one."""
    request = urllib.request.Request(
        base.rstrip("/") + "/auth/session",
        data=b"",
        method="POST",
        headers={"Authorization": "Bearer " + token},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            headers = response.headers.get_all("Set-Cookie") or []
    except urllib.error.HTTPError as error:
        if error.code == 404:
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
            say(tr("ankiquest: could not sign in to the website (%s)", language) % error)
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
