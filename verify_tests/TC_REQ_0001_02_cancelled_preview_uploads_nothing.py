from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0001_02():
    """TC-REQ-0001-02: leaving the first-sync preview without starting uploads nothing to Drive."""
    require_release_app()
    name = f"0vt-0001-02-{run_id()}"
    make_local_folder(name, {"a.txt": "alpha", "b.txt": "bravo"})
    try:
        create_pair_to_preview(name)
        tap_tag("dryrun_cancel")
        time.sleep(20)
        open_pair(name)
        assert side_count("drive") == 0
    finally:
        cleanup(name)
