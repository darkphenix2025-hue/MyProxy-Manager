# 安全审查第二轮修复计划（R2）

## 1. 背景

第一轮安全修复已完成并落地。第二轮复审发现仍有少量高风险与中风险问题，需要单独收敛，避免与已完成事项混淆。

本文件仅覆盖 R2 新增/遗留问题，不重复第一轮已关闭任务。

## 2. 修复目标

- 修复 OAuth Web 回传在不同端口场景下的消息投递失败问题
- 完成 OAuth `state` 的真正一次性消费，阻断重放窗口
- 提升安全测试有效性，避免“弱测试”误判通过
- 评估并落实 OAuth 密钥本地存储加固方案

## 3. 风险分级

### P0（必须立即修复）

1. OAuth 回调页 `postMessage` 目标域固定，可能导致前端收不到成功消息

### P1（本迭代内修复）

1. `state` 校验通过后未失效，存在重放窗口
2. CORS/鉴权相关测试断言不足，回归门禁有效性弱

### P2（增强项）

1. `oauth_client_secret` 在本地配置中以明文存储，需安全加固

## 4. 可执行任务清单

- [ ] `R2-SEC-001` 修复 OAuth 回传消息目标域（P0，2h）
  - 范围：`src-tauri/src/proxy/server.rs`、`src/components/accounts/AddAccountDialog.tsx`
  - 变更：
    - 回调页 `postMessage` 使用受信任来源策略，兼容 `localhost:1420` 与 `localhost:8045`。
    - 前端 `message` 校验由宽松匹配改为精确白名单（含端口）。
  - 依赖：无
  - 完成定义：
    - 开发模式与生产模式均可自动完成 OAuth 成功回传。
    - 非白名单来源消息无法触发成功逻辑。

- [ ] `R2-SEC-002` 实现 `state` 单次消费（P1，2h）
  - 范围：`src-tauri/src/modules/oauth_server.rs`
  - 变更：
    - `validate_web_oauth_state` 命中后立即失效（清理或标记 consumed）。
    - 禁止同一 `state` 二次通过。
  - 依赖：无
  - 完成定义：
    - 首次正确 `state` 可通过，第二次同值必失败。
    - 无活跃 flow 时一律拒绝。

- [ ] `R2-SEC-003` 强化安全回归测试（P1，4h）
  - 范围：`src-tauri/src/proxy/tests/`、`src-tauri/src/proxy/middleware/cors.rs`、`src-tauri/src/modules/oauth_server.rs`
  - 变更：
    - 补齐 CORS 恶意来源拒绝测试（带明确断言）。
    - 增加 OAuth 回传成功/失败、`state` 重放失败测试。
    - 清理“仅创建对象即通过”的弱测试。
  - 依赖：`R2-SEC-001`、`R2-SEC-002`
  - 完成定义：
    - 新增测试可稳定复现并拦截对应问题。
    - 安全相关测试集全绿。

- [ ] `R2-SEC-004` OAuth 密钥存储加固方案（P2，0.5d Spike + 0.5d 实施）
  - 范围：`src-tauri/src/models/config.rs`、配置读写模块、设置页
  - 变更：
    - 评估并选型：系统密钥链优先；若短期不可行，落地加密存储与文件权限约束。
    - 完成迁移策略与向后兼容读取逻辑。
  - 依赖：无
  - 完成定义：
    - 产出决策记录与实施结果。
    - 日志/导出路径不泄露密钥明文。

## 5. 执行顺序

1. 先做 `R2-SEC-001`（直接影响 OAuth 可用性）
2. 再做 `R2-SEC-002`（关闭安全重放窗口）
3. 然后做 `R2-SEC-003`（固化回归门禁）
4. 最后做 `R2-SEC-004`（安全增强与技术债收敛）

## 6. 验收清单（发布前）

- [ ] OAuth Web 登录在 `localhost:1420` 与 `localhost:8045` 场景均可自动完成
- [ ] 伪造 `postMessage`、错误 `state`、重放 `state` 均被拒绝
- [ ] R2 新增安全测试全部通过
- [ ] 密钥存储策略明确且已文档化

## 7. 建议分支与提交粒度

- 分支：`fix/security-r2-oauth-hardening`
- 提交建议：
  - `fix(oauth): stabilize postMessage target/origin handling`
  - `fix(oauth): enforce one-time state consumption`
  - `test(security): add cors/oauth replay regression tests`
  - `chore(security): harden oauth secret storage`
