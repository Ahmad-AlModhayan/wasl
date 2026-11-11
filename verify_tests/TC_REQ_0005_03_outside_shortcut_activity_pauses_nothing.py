from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0005_03():
    """TC-REQ-0005-03: an outside launch of ShortcutActivity with pause_all pauses nothing."""
    require_release_app()
    name = f"0vt-0005-03-{run_id()}"
    make_local_folder(name, {"a.txt": "alpha"})
    try:
        create_pair_to_preview(name)
        tap_tag("dryrun_cancel")
        sh(f"am start -n {PACKAGE}/.share.ShortcutActivity --es action pause_all")
        time.sleep(8)
        open_pair(name)
        assert find("pair_pause") is not None and find("pair_resume") is None
    finally:
        cleanup(name)
