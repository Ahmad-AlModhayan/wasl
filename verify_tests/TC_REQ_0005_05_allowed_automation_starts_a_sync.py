from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0005_05():
    """TC-REQ-0005-05: the user's allowed automation can still start a sync."""
    require_release_app()
    name = f"0vt-0005-05-{run_id()}"
    try:
        secret = automation(True)
        first_sync(name, {"a.txt": "alpha"})
        broadcast("SYNC_ALL", secret)
        time.sleep(40)
        open_pair(name)
        assert history_has("intent")
    finally:
        try:
            cleanup(name)
        finally:
            automation(False)
