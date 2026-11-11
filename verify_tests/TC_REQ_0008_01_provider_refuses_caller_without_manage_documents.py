"""TC-REQ-0008-01 (target: android): only holders of MANAGE_DOCUMENTS may read Drive via the provider."""
from _gate import adb, require_release_app


def test_TC_REQ_0008_01():
    require_release_app()
    r = adb("shell", "content", "query", "--uri", "content://com.almodhayan.drivesync.documents/root")
    out = r.stdout + r.stderr
    assert "SecurityException" in out, f"provider answered a caller without MANAGE_DOCUMENTS:\n{out[:1500]}"
    assert "Row:" not in out, f"provider returned rows to an unprivileged caller:\n{out[:1500]}"
