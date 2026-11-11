"""TC_REQ_0004_02 (target: unit) -> com.almodhayan.drivesync.sync.PlannerTest.conflict_bothChangedToIdenticalContent_adopts"""
from _gate import gradle_unit


def test_TC_REQ_0004_02():
    gradle_unit("com.almodhayan.drivesync.sync.PlannerTest", "conflict_bothChangedToIdenticalContent_adopts")
