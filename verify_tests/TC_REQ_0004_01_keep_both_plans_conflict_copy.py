"""TC_REQ_0004_01 (target: unit) -> com.almodhayan.drivesync.sync.PlannerTest.conflict_keepBoth_producesConflictAction"""
from _gate import gradle_unit


def test_TC_REQ_0004_01():
    gradle_unit("com.almodhayan.drivesync.sync.PlannerTest", "conflict_keepBoth_producesConflictAction")
