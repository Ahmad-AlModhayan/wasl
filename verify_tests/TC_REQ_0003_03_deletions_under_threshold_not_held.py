"""TC_REQ_0003_03 (target: unit) -> com.almodhayan.drivesync.sync.PlannerTest.massDeleteGuard_notBlockedBelowMinFiles"""
from _gate import gradle_unit


def test_TC_REQ_0003_03():
    gradle_unit("com.almodhayan.drivesync.sync.PlannerTest", "massDeleteGuard_notBlockedBelowMinFiles")
