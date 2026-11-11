from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0009_01():
    """TC-REQ-0009-01: the first pair created in a fresh app session syncs a phone edit within 30 s."""
    require_release_app()
    go_home()
    existing = [n.rid for n in dump() if n.rid.startswith("pair_card:")]
    assert not existing, f"environment: needs a phone with no pairs, found {existing}"
    launch(cold=True)
    name = f"0vt-0009-01-{run_id()}"
    try:
        path = first_sync(name, {"a.txt": "alpha"})
        sh(f"printf ' edited' >> {path}/a.txt")
        time.sleep(30)
        open_pair(name)
        assert history_has("instant")
    finally:
        cleanup(name)
