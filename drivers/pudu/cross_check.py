"""The signing cross-check: the vendor's own script against this fake's verifier.

`run-check.sh` runs this with the skill repo's path. It imports Pudu's `pudu-request.py`
unmodified, captures what it signs (its `requests` calls are stubbed — nothing leaves the
machine), and feeds the captured date, auth header, path, query and body into the fake cloud's
`_verify`. One GET and one POST, the POST carrying CJK bytes — the case where `ensure_ascii`
decides the signature.

If the fake accepts the vendor's construction, and the suite accepts the driver against the fake,
the driver's signing matches the vendor's through two independent implementations. Either half
failing is a defect in this tree, not in theirs.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                               "bindings", "python"))
from skynet_pudu.fake_cloud import TEST_KEY, TEST_SECRET, _verify


class _Captured:
    method = ""
    url = ""
    headers: dict = {}
    body = b""


class _FakeResponse:
    status_code = 200

    def json(self):  # the vendor script reads message == "SUCCESS" off this
        return {"message": "SUCCESS", "data": {}}


def _capture(method: str):
    def call(url, headers=None, data=None, timeout=None):
        _Captured.method = method
        _Captured.url = url
        _Captured.headers = headers or {}
        _Captured.body = data if isinstance(data, bytes) else b""
        return _FakeResponse()

    return call


def main() -> int:
    skill = sys.argv[1] if len(sys.argv) > 1 else ""
    script = os.path.join(skill, "skills", "pudu-openapi-skill", "scripts", "pudu-request.py")
    if not os.path.isfile(script):
        print(f"cross-check: no vendor script at {script}. Clone the skill repo:\n"
              f"  git clone --depth 1 https://github.com/pudu-robotics/skills <dir>\n"
              f"and pass <dir>.", flush=True)
        return 2
    spec = importlib.util.spec_from_file_location("pudu_request_vendor", script)
    assert spec and spec.loader
    vendor = importlib.util.module_from_spec(spec)
    # The capture happens before import finishes mattering: stub `requests` in their namespace
    # right after exec by patching the module object they imported.
    spec.loader.exec_module(vendor)
    vendor.requests.get = _capture("GET")
    vendor.requests.post = _capture("POST")

    failures = 0

    def check(held: bool, what: str) -> None:
        nonlocal failures
        print(("  ok    " if held else "  FAIL  ") + what, flush=True)
        if not held:
            failures += 1

    # A GET, signed by their code with the test credentials.
    vendor.request("/open-platform-service/v1/robot/get_position", "GET",
                   params={"sn": "SN-TEST-1"}, hostname="127.0.0.1",
                   api_app_key=TEST_KEY, api_app_secret=TEST_SECRET)
    check(_verify("GET", "/open-platform-service/v1/robot/get_position", "sn=SN-TEST-1",
                  b"", _Captured.headers),
          "the fake accepts the vendor's GET signature")

    # A POST with CJK bytes — the ensure_ascii case.
    vendor.request("/open-platform-service/v1/delivery_task", "POST",
                   params={"sn": "SN-TEST-1",
                           "payload": {"trays": [{"destinations": [{"points": "吧台"}]}]}},
                   hostname="127.0.0.1",
                   api_app_key=TEST_KEY, api_app_secret=TEST_SECRET)
    check(_verify("POST", "/open-platform-service/v1/delivery_task", "", _Captured.body,
                  _Captured.headers),
          "the fake accepts the vendor's POST signature, CJK bytes and all")

    # And the negative: tamper one byte and the same verifier must refuse.
    tampered = _Captured.body[:-1] + (b"}" if not _Captured.body.endswith(b"}") else b"X")
    check(not _verify("POST", "/open-platform-service/v1/delivery_task", "", tampered,
                      _Captured.headers),
          "and refuses the same request with one tampered byte")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
