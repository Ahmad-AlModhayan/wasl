from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0001_04():
    """TC-REQ-0001-04: a sync-all trigger does not bypass a pair still waiting for its preview."""
    require_release_app()
    name = f"0vt-0001-04-{run_id()}"
    make_local_folder(name, {"a.txt": "alpha", "b.txt": "bravo"})
    try:
        create_pair_to_preview(name)
        tap_tag("dryrun_cancel")
        go_home()
        tap_tag("home_sync_all")
        time.sleep(30)
        open_pair(name)
        assert side_count("drive") == 0
    finally:
        cleanup(name)
