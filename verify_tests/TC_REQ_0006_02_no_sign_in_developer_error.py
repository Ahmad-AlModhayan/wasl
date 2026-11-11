from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0006_02():
    """TC-REQ-0006-02: using the release build logs no Google sign-in developer error."""
    require_release_app()
    adb("logcat", "-c")
    go_home()
    tap_tag("home_account")
    wait_for("account_storage", 30)
    time.sleep(5)
    log = adb("logcat", "-d", timeout=120).stdout
    bad = [l for l in log.splitlines() if "DEVELOPER_ERROR" in l or "ApiException: 10:" in l]
    assert not bad, "\n".join(bad[:5])
