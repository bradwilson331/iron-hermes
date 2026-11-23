# Phase 49.8 D-17 (local layer): opt-in Humble + rosbridge + rosapi +
# turtlesim harness. Official ROS image + official apt packages only — never
# part of the CI gate (T-49.8-SC, `<threat_model>` in 49.8-10-PLAN.md).
#
# Proves the apt Humble rosbridge build end to end (D-06), including action
# goals — the mock server in crates/ironhermes-tools/tests/ proves protocol
# logic; only a real rosbridge proves this.
# Multi-arch INDEX digest (amd64 + arm64), resolved 2026-11-01. Re-resolve: GET
# the tag's manifest from registry-1.docker.io with the OCI-index Accept header,
# read `docker-content-digest`, confirm with `podman manifest inspect`.
FROM docker.io/library/ros:humble-ros-base@sha256:d9bcf2e6f72df127fdee07048c2e87b1b4b9a59a74af0592f4489fc2844aa855

RUN apt-get update && apt-get install -y --no-install-recommends \
        ros-humble-rosbridge-suite \
        ros-humble-rosapi \
        ros-humble-turtlesim \
    && rm -rf /var/lib/apt/lists/*

# turtlesim needs a Qt platform plugin; there is no X server in the
# container, so run it headless.
ENV QT_QPA_PLATFORM=offscreen

# Prints the installed rosbridge version (D-06), starts turtlesim_node in
# the background, then launches rosbridge_server bound to all interfaces
# INSIDE the container — host exposure is limited to 127.0.0.1 only by
# docker-compose.yml's port mapping, not by anything in this image.
# Note: `-u` (unbound-variable checking) is deliberately NOT enabled here —
# /opt/ros/humble/setup.bash itself references unset variables
# (AMENT_TRACE_SETUP_FILES and friends) and aborts under `set -u`; this is a
# known property of ROS's own setup scripts, not something this image's
# script can avoid while still sourcing them.
CMD ["/bin/bash", "-c", "\
set -eo pipefail; \
source /opt/ros/humble/setup.bash; \
echo 'Installed rosbridge version:'; \
dpkg -s ros-humble-rosbridge-server | grep Version; \
ros2 run turtlesim turtlesim_node & \
sleep 2; \
exec ros2 launch rosbridge_server rosbridge_websocket_launch.xml port:=9090 address:=0.0.0.0 \
"]
