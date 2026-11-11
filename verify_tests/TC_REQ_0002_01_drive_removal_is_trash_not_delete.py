"""TC_REQ_0002_01 (target: unit) -> com.almodhayan.drivesync.drive.DriveApiTest.trashUntrashRename_sendMinimalPatchBodies"""
from _gate import gradle_unit


def test_TC_REQ_0002_01():
    gradle_unit("com.almodhayan.drivesync.drive.DriveApiTest", "trashUntrashRename_sendMinimalPatchBodies")
