"""Signing in and creating accounts from the desktop add-on, against a stand-in server."""

import ast
import importlib.util
import json
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "addon"


def load(name):
    spec = importlib.util.spec_from_file_location("aq_accounts_" + name, ROOT / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class Server(BaseHTTPRequestHandler):
    received = []

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        Server.received.append((self.path, body))
        if self.path == "/api/accounts/tokens" and body["password"] == "right password":
            self.answer(200, {"user": body["user"], "display": "Ana", "token": "device-token"})
        elif self.path == "/api/accounts/tokens":
            self.answer(401, {"error": "Username or password not recognized."})
        elif self.path == "/api/accounts" and body["user"] == "taken":
            self.answer(409, {"error": "That username is taken."})
        elif self.path == "/api/accounts":
            self.answer(201, {"user": body["user"], "display": body.get("display", body["user"]), "token": "first-token"})
        else:
            self.answer(404, None)

    def answer(self, status, body):
        payload = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_):
        pass


class AccountTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.client = load("client")
        cls.server = HTTPServer(("127.0.0.1", 0), Server)
        cls.base = "http://127.0.0.1:%d/" % cls.server.server_port
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        Server.received.clear()

    def test_signing_in_trades_the_password_for_a_device_token(self):
        answer = self.client.sign_in(self.base, "ana", "right password", "Anki desktop (Linux)")
        self.assertEqual(answer["token"], "device-token")
        self.assertEqual(Server.received, [("/api/accounts/tokens", {"user": "ana", "password": "right password", "device": "Anki desktop (Linux)"})])

    def test_refusals_carry_the_status_and_the_servers_message(self):
        with self.assertRaises(self.client.AccountError) as refused:
            self.client.sign_in(self.base, "ana", "wrong", "Anki desktop")
        self.assertEqual((refused.exception.status, refused.exception.message), (401, "Username or password not recognized."))
        with self.assertRaises(self.client.AccountError) as taken:
            self.client.sign_up(self.base, "taken", "long enough pw", "", "Anki desktop")
        self.assertEqual(taken.exception.status, 409)

    def test_signing_up_sends_the_display_name_only_when_given(self):
        self.client.sign_up(self.base, "ana", "long enough pw", "", "Anki desktop")
        self.client.sign_up(self.base, "bo", "long enough pw", "Bo", "Anki desktop")
        self.assertNotIn("display", Server.received[0][1])
        self.assertEqual(Server.received[1][1]["display"], "Bo")

    def test_the_deck_list_invites_unconnected_players_to_sign_in(self):
        board = load("board")
        self.assertIn("pycmd('ankiquest:account')", board.welcome())
        self.assertIn("Inicia sesión", board.welcome(lambda text: load("language").tr(text, "es")))


class SpanishCoverage(unittest.TestCase):
    def test_every_desktop_label_has_a_spanish_translation(self):
        spanish = load("language").SPANISH
        labels = {
            node.args[0].value
            for path in ROOT.glob("*.py")
            for node in ast.walk(ast.parse(path.read_text("utf-8")))
            if isinstance(node, ast.Call)
            and getattr(node.func, "id", None) in ("tr", "translate")
            and node.args
            and isinstance(node.args[0], ast.Constant)
            and isinstance(node.args[0].value, str)
        }
        missing = sorted(labels - set(spanish))
        self.assertEqual(missing, [])


if __name__ == "__main__":
    unittest.main()
