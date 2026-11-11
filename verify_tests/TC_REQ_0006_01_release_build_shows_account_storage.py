from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0006_01():
    """TC-REQ-0006-01: the release build shows the signed-in test account with its Drive storage."""
    require_release_app()
    go_home()
    tap_tag("home_account")
    assert wait_for("account_storage", 30) is not None
