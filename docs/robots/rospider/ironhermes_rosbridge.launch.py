#!/usr/bin/env python3
"""ironhermes_rosbridge.launch.py

Phase 49.8 D-18 — robot-side deliverable for ironhermes's ROS 2 integration with the
Hiwonder ROSPider: docs + this launch file only. No ironhermes code is ever deployed to
the Jetson; this launches only the standard rosbridge_suite server plus rosapi.

Starts rosbridge_websocket and rosapi_node by including rosbridge_server's own
launch/rosbridge_websocket_launch.xml, alongside the ROSPider's normal bringup
(launched separately, in its own terminal).

Two supported topologies (see docs/robots/rospider.md, D-02):
  - on-robot: ironhermes runs on the Jetson itself, default bind 127.0.0.1
      ros2 launch ironhermes_rosbridge.launch.py
  - LAN: ironhermes runs on the operator's machine, bind the LAN-reachable interface
      ros2 launch ironhermes_rosbridge.launch.py address:=<jetson-lan-ip>

The default bind address is always 127.0.0.1 — exposing rosbridge on the LAN is always
an explicit operator override, never the default (rosbridge has no authentication).
"""

from launch import LaunchDescription
from launch.actions import DeclareLaunchArgument, IncludeLaunchDescription
from launch.launch_description_sources import AnyLaunchDescriptionSource
from launch.substitutions import LaunchConfiguration
from ament_index_python.packages import get_package_share_directory

import os


def generate_launch_description():
    port_arg = DeclareLaunchArgument(
        'port',
        default_value='9090',
        description="TCP port rosbridge_websocket listens on.",
    )
    address_arg = DeclareLaunchArgument(
        'address',
        default_value='127.0.0.1',
        description=(
            "Interface rosbridge_websocket binds to. Defaults to localhost only — "
            "set explicitly (e.g. the Jetson's LAN IP) to expose it to the LAN. "
            "Never bind all interfaces; rosbridge has no authentication."
        ),
    )

    rosbridge_websocket_launch = os.path.join(
        get_package_share_directory("rosbridge_server"),
        "launch",
        "rosbridge_websocket_launch.xml",
    )

    rosbridge = IncludeLaunchDescription(
        AnyLaunchDescriptionSource(rosbridge_websocket_launch),
        launch_arguments={
            "port": LaunchConfiguration("port"),
            "address": LaunchConfiguration("address"),
        }.items(),
    )

    return LaunchDescription(
        [
            port_arg,
            address_arg,
            rosbridge,
        ]
    )
