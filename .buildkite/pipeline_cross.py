#!/usr/bin/env python3
# Copyright 2023 Amazon.com, Inc. or its affiliates. All Rights Reserved.
# SPDX-License-Identifier: Apache-2.0

"""Generate Buildkite Cross Snapshot/Restore pipelines dynamically

1. Generate snapshots for each instance and kernel version
2. wait
3. Restore snapshots across instances and kernels
"""

import itertools

from common import DEFAULT_PLATFORMS, group, pipeline_to_json


def restore_step(label, src_instance, src_kv, dst_instance, dst_os, dst_kv):
    """Generate a restore step"""
    pytest_keyword_for_instance = {
        "c5n.metal": "-k 'not None'",
        "m5n.metal": "-k 'not None'",
        "m6i.metal": "-k 'not None'",
        "m6a.metal": "",
    }
    k_val = pytest_keyword_for_instance.get(dst_instance, "")
    return {
        "command": [
            f"buildkite-agent artifact download snapshots/{src_instance}_{src_kv}/* .",
            f"mv -v snapshots/{src_instance}_{src_kv} snapshot_artifacts",
            f"./tools/devtool -y test -- -m nonci {k_val} integration_tests/functional/test_snapshot_restore_cross_kernel.py",
        ],
        "label": label,
        "timeout": 30,
        "agents": {"instance": dst_instance, "kv": dst_kv, "os": dst_os},
    }


def cross_steps():
    """Generate group steps"""
    instances_x86_64 = ["c5n.metal", "m5n.metal", "m6i.metal", "m6a.metal"]
    instances_aarch64 = ["m7g.metal"]
    groups = []
    commands = [
        "./tools/devtool -y build --release",
        "./tools/devtool -y sh ./tools/create_snapshot_artifact/main.py",
        "mkdir -pv snapshots/{instance}_{kv}",
        "sudo chown -Rc $USER: snapshot_artifacts",
        "mv -v snapshot_artifacts/* snapshots/{instance}_{kv}",
    ]
    groups.append(
        group(
            "📸 create snapshots",
            commands,
            timeout=30,
            artifact_paths="snapshots/**/*",
            instances=instances_x86_64,
            platforms=DEFAULT_PLATFORMS,
        )
    )
    groups.append("wait")

    # allow-list of what instances can be restores on what other instances (in
    # addition to itself)
    supported = {
        "c5n.metal": ["m5n.metal", "m6i.metal"],
        "m5n.metal": ["c5n.metal", "m6i.metal"],
        "m6i.metal": ["c5n.metal", "m5n.metal"],
    }

    # https://github.com/firecracker-microvm/firecracker/blob/main/docs/kernel-policy.md#experimental-snapshot-compatibility-across-kernel-versions
    aarch64_platforms = [("al2023", "linux_6.1")]
    perms_aarch64 = itertools.product(
        instances_aarch64, aarch64_platforms, instances_aarch64, aarch64_platforms
    )

    perms_x86_64 = itertools.product(
        instances_x86_64, DEFAULT_PLATFORMS, instances_x86_64, DEFAULT_PLATFORMS
    )
    steps = []
    for (
        src_instance,
        (_, src_kv),
        dst_instance,
        (dst_os, dst_kv),
    ) in itertools.chain(perms_x86_64, perms_aarch64):
        # the integration tests already test src == dst, so we skip it
        if src_instance == dst_instance and src_kv == dst_kv:
            continue
        # newer -> older is not supported, and does not work
        if src_kv > dst_kv:
            continue
        if src_instance != dst_instance and dst_instance not in supported.get(
            src_instance, []
        ):
            continue

        step = restore_step(
            f"🎬 {src_instance} {src_kv} ➡️ {dst_instance} {dst_kv}",
            src_instance,
            src_kv,
            dst_instance,
            dst_os,
            dst_kv,
        )
        steps.append(step)
    groups.append({"group": "🎬 restore across instances and kernels", "steps": steps})
    return groups


if __name__ == "__main__":
    pipeline = {"steps": cross_steps()}
    print(pipeline_to_json(pipeline))
