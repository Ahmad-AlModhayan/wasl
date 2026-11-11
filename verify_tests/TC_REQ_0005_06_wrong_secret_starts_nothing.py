from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0005_06():
    """TC-REQ-0005-06: an automation request with the wrong secret starts no sync."""
    require_release_app()
    name = f"0vt-0005-06-{run_id()}"
    try:
        automation(True)
        first_sync(name, {"a.txt": "alpha"})
        broadcast("SYNC_ALL", "0" * 32)
        time.sleep(30)
        open_pair(name)
        assert not history_has("intent")
    finally:
        try:
            cleanup(name)
        finally:
            automation(False)
