from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0005_04():
    """TC-REQ-0005-04: an allowed automation sync still respects Require VPN."""
    require_release_app()
    name = f"0vt-0005-04-{run_id()}"
    try:
        secret = automation(True)
        path = first_sync(name, {"a.txt": "alpha"})
        open_pair(name)
        tap_tag("pair_edit")
        tap(reveal("editor_require_vpn", "editor_more_conditions"))
        tap_tag("editor_save")
        time.sleep(2)
        sh(f"printf new > {path}/new.txt")
        time.sleep(5)
        broadcast("SYNC_ALL", secret)
        time.sleep(40)
        open_pair(name)
        assert side_count("drive") == 1
    finally:
        try:
            cleanup(name)
        finally:
            automation(False)
