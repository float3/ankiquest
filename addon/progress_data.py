"""Progress reads the same owner-only history and visible records as the website."""
from enum import Enum, auto
from urllib.parse import quote


class _Section(Enum):
    HISTORY = auto()
    RECORDS = auto()


def _fetch(api, section, year=None):
    if section is _Section.RECORDS:
        return api._request("/api/records")
    path = "/api/study/" + quote(api.user, safe="")
    if year is not None:
        path += "?year=%d" % year
    return api._request(path)
