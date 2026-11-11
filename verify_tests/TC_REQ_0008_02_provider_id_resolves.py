"""TC_REQ_0008_02 (target: unit) -> com.almodhayan.drivesync.ondemand.DocumentIdsTest.roundTripsThroughTheSharedConvention"""
from _gate import gradle_unit


def test_TC_REQ_0008_02():
    gradle_unit("com.almodhayan.drivesync.ondemand.DocumentIdsTest", "roundTripsThroughTheSharedConvention")
