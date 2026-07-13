# 安全审查修复任务计划

## 1. 目标与范围

本计划用于修复本次代码审查识别出的高风险安全问题，目标是：

- 关闭管理接口跨站调用风险
- 修复 OAuth 回调 CSRF 风险
- 移除源码中的敏感密钥硬编码
- 补齐关键安全路径回归测试，防止后续回归

本计划覆盖后端 `src-tauri` 与前端 `src` 的鉴权、CORS、OAuth 相关模块。

## 2. 风险与优先级

### P0（必须优先完成）

1. 管理接口在默认本地模式下可被跨站调用
2. `/auth/callback` 未强制校验 OAuth `state`
3. OAuth `client_secret` 硬编码在源码

### P1（P0 完成后立即处理）

1. 前端 `postMessage` 事件缺少 `origin/source` 校验
2. 鉴权路径自动化测试不足（存在占位测试）

## 3. 任务分解与验收标准

### 任务 A（P0）：收紧管理接口鉴权与 CORS

**涉及模块**

- `src-tauri/src/proxy/security.rs`
- `src-tauri/src/proxy/middleware/auth.rs`
- `src-tauri/src/proxy/middleware/cors.rs`
- `src-tauri/src/proxy/server.rs`

**实施项**

1. 管理接口改为“始终严格鉴权”，不受 `ProxyAuthMode::Off` 放行逻辑影响
2. 将 CORS 从 `allow_origin(Any)` 改为白名单策略
3. 管理接口与代理接口按路由维度拆分 CORS 策略（管理接口最小权限）
4. 明确本地 UI 场景下允许的 Origin 与 Header 清单

**验收标准**

- 未携带正确管理口令访问 `/api/*` 返回 `401/403`
- 任意第三方网页无法跨域读取/写入管理接口
- 本地管理 UI 的正常请求不受影响
- 相关集成测试通过

### 任务 B（P0）：修复 OAuth 回调 `state` 校验

**涉及模块**

- `src-tauri/src/proxy/server.rs`
- `src-tauri/src/modules/oauth_server.rs`

**实施项**

1. `handle_oauth_callback` 强制校验 `state` 存在且与服务端会话一致
2. 不匹配时返回明确错误并拒绝绑定账号
3. 成功后清理或失效一次性 `state`，防重放
4. 统一 Web 回调与手动提交流程的状态校验策略

**验收标准**

- 错误 `state` 无法完成账号绑定
- 缺失 `state` 直接失败
- 正常流程可成功完成 OAuth 登录
- 新增回归测试覆盖成功/失败分支

### 任务 C（P0）：移除硬编码 OAuth 密钥并完成轮换

**涉及模块**

- `src-tauri/src/modules/oauth.rs`
- 启动配置与文档（`README`/`docs`）

**实施项**

1. 删除源码内置 `CLIENT_SECRET` 常量
2. 改为环境变量或安全配置读取（启动时校验必填）
3. 为缺失配置提供可观测错误（日志与接口提示）
4. 完成历史密钥轮换与失效处理（运维动作）

**验收标准**

- 仓库中无有效明文生产密钥
- 缺少密钥配置时服务拒绝进入可用态并给出明确提示
- 配置正确时 OAuth 全流程可用
- 文档包含最小可运行配置示例

### 任务 D（P1）：前端 OAuth 消息源校验

**涉及模块**

- `src/components/accounts/AddAccountDialog.tsx`

**实施项**

1. `message` 事件中校验 `event.origin` 与预期来源
2. 校验 `event.source === popup`
3. 增加超时与异常路径提示，避免伪造成功状态

**验收标准**

- 非授权来源的 `postMessage` 不触发成功逻辑
- 正常 OAuth 回调仍能完成账号刷新

### 任务 E（P1）：补齐测试与安全回归门禁

**涉及模块**

- `src-tauri/src/proxy/tests/`
- `src-tauri/src/proxy/middleware/auth.rs`（单元测试）

**实施项**

1. 新增管理接口鉴权与 CORS 负向测试
2. 新增 OAuth `state` 校验测试
3. 清理无效占位测试，补充可执行断言
4. 在 CI 中纳入安全回归测试集合

**验收标准**

- 新增测试稳定通过
- 可复现本次漏洞场景并验证已被拦截

## 4. 建议执行顺序（含依赖）

1. 任务 C（密钥治理）与任务 A（鉴权/CORS）并行启动
2. 任务 B（OAuth `state`）紧随其后，避免新增绑定风险
3. 任务 D（前端消息校验）随后落地
4. 任务 E（测试门禁）收尾并作为发布前阻断条件

## 5. 发布前检查清单

- [ ] P0 全部完成并通过回归测试
- [ ] 风险复测：跨站调用、OAuth CSRF、密钥泄露场景均被阻断
- [ ] 文档更新完成（配置项、迁移说明、故障排查）
- [ ] 变更记录中明确安全修复项与潜在兼容性影响

## 6. 回滚与应急预案

- 保留鉴权与 CORS 变更前配置快照，支持快速回滚
- 对 OAuth 改动提供临时开关仅用于紧急止血（默认关闭）
- 若发布后出现登录异常，优先启用只读模式保护管理接口写操作

---

该计划可直接拆分为 issue/子任务执行，建议按 P0 -> P1 节奏推进，并以安全回归测试作为每个阶段的完成判据。

## 7. 可执行任务清单（可直接建 Issue）

### Sprint 1（P0，预计 2-3 天）

- [ ] `SEC-001` 管理接口强制鉴权逻辑修复（4h）
  - 范围：`src-tauri/src/proxy/middleware/auth.rs`
  - 动作：删除管理接口在 `ProxyAuthMode::Off` 下的放行路径；统一走管理口令校验
  - 依赖：无
  - 完成定义：未带口令访问 `/api/config`、`/api/accounts` 返回 `401/403`

- [ ] `SEC-002` CORS 白名单策略与路由分层（6h）
  - 范围：`src-tauri/src/proxy/middleware/cors.rs`、`src-tauri/src/proxy/server.rs`
  - 动作：将管理 API 与代理 API 拆分 CORS；管理 API 仅允许可信 Origin
  - 依赖：`SEC-001`
  - 完成定义：第三方站点跨域调用管理接口被浏览器拦截/服务端拒绝，本地 UI 正常

- [ ] `SEC-003` OAuth 回调 state 强校验（6h）
  - 范围：`src-tauri/src/proxy/server.rs`、`src-tauri/src/modules/oauth_server.rs`
  - 动作：`/auth/callback` 强制校验 `state`；不匹配拒绝绑定；成功后失效 state
  - 依赖：无
  - 完成定义：错误或缺失 state 无法完成绑定，正确 state 可完成登录

- [ ] `SEC-004` 移除硬编码 OAuth Secret（5h）
  - 范围：`src-tauri/src/modules/oauth.rs`、配置加载模块
  - 动作：改为环境变量/配置读取；缺失时启动报错并拒绝进入可用态
  - 依赖：无
  - 完成定义：仓库无明文 secret；本地配置正确时 OAuth 可用，缺失时给出明确错误

- [ ] `SEC-005` P0 回归测试补齐（6h）
  - 范围：`src-tauri/src/proxy/tests/`、`src-tauri/src/proxy/middleware/auth.rs`
  - 动作：新增管理鉴权、CORS、OAuth state 的正反向测试
  - 依赖：`SEC-001`、`SEC-002`、`SEC-003`
  - 完成定义：新增测试稳定通过，能复现并拦截本次漏洞路径

### Sprint 2（P1，预计 1-2 天）

- [ ] `SEC-006` 前端 OAuth postMessage 来源校验（3h）
  - 范围：`src/components/accounts/AddAccountDialog.tsx`
  - 动作：校验 `event.origin` 与 `event.source === popup`，拒绝非预期消息
  - 依赖：`SEC-003`
  - 完成定义：伪造 `postMessage` 不触发成功逻辑，真实回调流程不受影响

- [ ] `SEC-007` 清理占位测试并增强断言（4h）
  - 范围：`src-tauri/src/proxy/middleware/auth.rs`、安全相关测试文件
  - 动作：移除 `assert!(true)` 类测试，补充可执行行为断言
  - 依赖：`SEC-005`
  - 完成定义：关键测试不再是占位；失败时能准确暴露鉴权回归

- [ ] `SEC-008` 文档与发布说明更新（3h）
  - 范围：`docs/`、`README.md` 或 `docs/advanced_configuration.md`
  - 动作：补充新配置项、迁移说明、常见故障排查
  - 依赖：`SEC-004`、`SEC-006`
  - 完成定义：新同学可按文档完成安全配置并通过验证

## 8. 执行节奏与并行建议

- 并行组 A：`SEC-001` + `SEC-003`（后端鉴权/OAuth）
- 并行组 B：`SEC-004`（配置治理，独立推进）
- 收敛阶段：`SEC-002`（依赖鉴权策略确定）+ `SEC-005`
- 第二阶段：`SEC-006`、`SEC-007`、`SEC-008`

## 9. 每日推进模板（建议粘贴到站会）

- 今日完成：`SEC-xxx`
- 明日计划：`SEC-yyy`
- 当前阻塞：无 / `xxx`
- 风险项：是否影响 OAuth 登录、管理 UI、现有部署配置
- 需要协助：评审 / 测试环境 / 密钥轮换窗口

## 10. 合并门禁（DoD）

- [ ] `SEC-001` 到 `SEC-008` 全部完成
- [ ] 安全回归测试全绿（含负向场景）
- [ ] 关键路径手测通过（管理登录、账号 OAuth、代理转发）
- [ ] 文档与变更说明齐全
- [ ] 至少 1 次同级代码审查通过
