from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0001_01():
    """TC-REQ-0001-01: leaving the first-sync preview without starting leaves the phone folder unchanged."""
    require_release_app()
    name = f"0vt-0001-01-{run_id()}"
    path = make_local_folder(name, {"a.txt": "alpha", "b.txt": "bravo"})
    before = local_listing(path)
    try:
        create_pair_to_preview(name)
        tap_tag("dryrun_cancel")
        time.sleep(20)
        assert local_listing(path) == before
    finally:
        cleanup(name)
