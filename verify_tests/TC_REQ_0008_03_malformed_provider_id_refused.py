"""TC_REQ_0008_03 (target: unit) -> com.almodhayan.drivesync.ondemand.DocumentIdsTest.rejectsMalformedIds"""
from _gate import gradle_unit


def test_TC_REQ_0008_03():
    gradle_unit("com.almodhayan.drivesync.ondemand.DocumentIdsTest", "rejectsMalformedIds")
