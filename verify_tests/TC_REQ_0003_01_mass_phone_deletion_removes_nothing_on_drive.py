from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0003_01():
    """TC-REQ-0003-01: deleting 21 of 25 synced phone files removes nothing from Drive."""
    require_release_app()
    name = f"0vt-0003-01-{run_id()}"
    try:
        path = first_sync(name, {f"f{i:02d}.txt": f"file {i}" for i in range(25)})
        sh(f"cd {path} && rm " + " ".join(f"f{i:02d}.txt" for i in range(21)))
        open_pair(name)
        tap_tag("pair_sync_now")
        time.sleep(40)
        open_pair(name)
        assert side_count("drive") == 25
    finally:
        cleanup(name)
