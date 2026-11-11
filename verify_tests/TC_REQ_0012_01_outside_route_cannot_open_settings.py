from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0012_01():
    """TC-REQ-0012-01: an outside intent cannot deep-link into internal screens."""
    require_release_app()
    launch(route="settings", cold=True)
    wait_for("home_sync_all", 20)
    assert find("settings_automation") is None
