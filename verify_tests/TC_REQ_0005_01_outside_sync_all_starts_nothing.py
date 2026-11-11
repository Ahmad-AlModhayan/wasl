from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0005_01():
    """TC-REQ-0005-01: an outside SYNC_ALL broadcast starts no sync."""
    require_release_app()
    name = f"0vt-0005-01-{run_id()}"
    try:
        first_sync(name, {"a.txt": "alpha"})
        broadcast("SYNC_ALL")
        time.sleep(30)
        open_pair(name)
        assert not history_has("intent")
    finally:
        cleanup(name)
