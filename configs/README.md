# sdkwork-kernel Configs

Static, git-tracked reference configuration for the kernel workspace.

This directory holds checked-in configuration **templates and defaults** that
describe the kernel's own structure and tooling. It does not hold environment
values: concrete deployable-root values (environment, runtime, topology,
deployment) are owned by [`etc/`](../etc/) per `SOURCE_CONFIG_SPEC.md`, and
production secrets are injected through the target secret manager — checked-in
credentials are forbidden.

Contents:

- Kernel workspace reference configuration aligned with
  `sdkwork.app.config.json` and `specs/topology.spec.json`.

Related authority:

- `../specs/kernel-local-conventions.md`
- `../sdkwork-specs/SOURCE_CONFIG_SPEC.md`
