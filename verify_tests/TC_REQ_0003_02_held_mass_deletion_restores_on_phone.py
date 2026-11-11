from _gate import *  # noqa: F401,F403  (adb + testTag driver)


def test_TC_REQ_0003_02():
    """TC-REQ-0003-02: a held mass deletion can be restored on the phone."""
    require_release_app()
    name = f"0vt-0003-02-{run_id()}"
    try:
        path = first_sync(name, {f"f{i:02d}.txt": f"file {i}" for i in range(25)})
        sh(f"cd {path} && rm " + " ".join(f"f{i:02d}.txt" for i in range(21)))
        open_pair(name)
        tap_tag("pair_sync_now")
        time.sleep(40)
        go_home()
        card = scroll_to(f"pair_card:{name}")
        action = None
        for _ in range(4):   # the floating tab bar covers the card's last row until the list scrolls
            nodes = dump()
            card = find(f"pair_card:{name}", nodes) or card
            bar = find("nav_pairs", nodes)
            action = next((n for n in nodes if n.rid == "pair_card_action" and card.contains(n) and (bar is None or n.y2 <= bar.y1)), None)
            if action is not None:
                break
            swipe_up()
            time.sleep(0.8)
        assert action is not None, "the held pair offers no review action on its card"
        tap(action)
        tap_tag("review_restore", timeout=30)
        tap_tag("review_done", timeout=120)
        assert len(local_listing(path)) == 25
    finally:
        cleanup(name)
