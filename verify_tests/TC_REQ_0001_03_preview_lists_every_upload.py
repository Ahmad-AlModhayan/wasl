from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0001_03():
    """TC-REQ-0001-03: the first-sync preview lists every file the sync would upload."""
    require_release_app()
    name = f"0vt-0001-03-{run_id()}"
    make_local_folder(name, {"a.txt": "alpha", "b.txt": "bravo", "c.txt": "charlie"})
    try:
        create_pair_to_preview(name)
        assert count_in("summary_upload") == 3
        tap_tag("dryrun_cancel")
    finally:
        cleanup(name)
