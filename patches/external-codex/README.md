# external/codex 本地补丁

`external/codex` 是 **第三方子模块**（`https://github.com/openai/codex.git`），
本工作区在它的工作区里带着一批**未提交、也无法推送上游**的本地改动。
本目录把那份改动固化成可重放的补丁，避免它只存在于某个人的工作区里。

## 为什么不能直接提交进子模块

- 子模块处于 **detached HEAD**，没有可推送的分支；
- 上游是 `openai/codex`，本组织**没有 push 权限**；
- 若在子模块内新建提交并把指针写进 `sdkwork-kernel`，其他人在
  `git submodule update` 时会指向一个**远端不存在的 commit**，直接拉取失败。

所以正确做法是：**补丁进 kernel 仓库，子模块工作区保持原样不动。**

## 现状

| 项目 | 值 |
| --- | --- |
| 子模块路径 | `external/codex` |
| 上游 | `https://github.com/openai/codex.git` |
| base commit | `5af85998c24fb3353ddd8164c3ed472057b03cb3` |
| 补丁文件 | `0001-network-proxy-local.patch` |
| 补丁 sha256 | `825b77ad41f184f851e0670e694a1aceee5daae6204c999e6ada574d88a900fa` |
| 规模 | 11 个文件，+81 / −97 |
| 涉及范围 | 仅 `codex-rs/network-proxy/` |

改动文件：

```
codex-rs/network-proxy/Cargo.toml
codex-rs/network-proxy/src/attribution.rs
codex-rs/network-proxy/src/certs.rs
codex-rs/network-proxy/src/connect_policy.rs
codex-rs/network-proxy/src/http_proxy.rs
codex-rs/network-proxy/src/mitm.rs
codex-rs/network-proxy/src/mitm_tests.rs
codex-rs/network-proxy/src/native_certs.rs
codex-rs/network-proxy/src/socks5.rs
codex-rs/network-proxy/src/upstream.rs
codex-rs/network-proxy/src/upstream_tests.rs
```

## 校验记录（导出时实测）

- **反向校验**：`git apply --check --reverse 0001-network-proxy-local.patch`
  在带改动的工作区上通过 → 补丁与工作区改动逐字节一致，无遗漏、无多余。
- **正向校验**：在临时索引 `git read-tree HEAD` 后
  `git apply --cached --check 0001-network-proxy-local.patch` 通过
  → 补丁可在原始 base commit 上无冲突重放。

## 如何重放

在 `sdkwork-kernel` 仓库根目录执行：

```bash
git -C external/codex am ../../sdkwork-kernel/patches/external-codex/0001-network-proxy-local.patch
```

若子模块是干净 checkout、只想落到工作区而不生成提交：

```bash
git -C external/codex apply patches/external-codex/0001-network-proxy-local.patch
```

重放前请确认子模块 HEAD 仍是上表中的 base commit，否则可能产生冲突。

## 如何更新这份补丁

当 `external/codex` 工作区里的改动发生变化后，重新导出并重跑两向校验：

```bash
git -C external/codex diff > patches/external-codex/0001-network-proxy-local.patch
git -C external/codex apply --check --reverse patches/external-codex/0001-network-proxy-local.patch
sha256sum patches/external-codex/0001-network-proxy-local.patch
```

同时更新本文件里的 sha256 与规模数字。

## 已知状态

导出于 2026-10-23。补丁落地后，`external/codex` **工作区仍保持 dirty**
（`git -C sdkwork-kernel status` 会显示 `M external/codex`），这是**有意为之**：
它承载的是正在使用的本地改动，清掉会丢功能。需要恢复干净时再用上面的命令重放。
