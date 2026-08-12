# Codex 优先的多认证账号平台需求说明

> 状态：Draft / 已确认产品方向，认证接入细节待 Spike 验证
> 日期：2026-08-12
> 范围：账号、凭据、供应商连接、协议能力、路由与旧数据迁移
> 关联设计：[`docs/architecture/design-myproxy-manager.md`](../architecture/design-myproxy-manager.md)

## 实施状态

截至 2026-08-13，P0 基础切片已开始实施：

- 已建立 schema v3 的 Identity、Credential、SecretRef、CredentialSummary、ProviderConnection、WireProtocol 和生命周期类型；v3 持久化包络携带数值版本、拒绝错版本，并以 `auth_kind` 判别凭据载荷；
- 已将运行时 Credential 与公开 DTO 分离：普通 API 仅序列化 CredentialSummary，SecretRef 只通过显式 v3 持久化边界写入；SecretRef/TokenResponse 的 Debug 输出和 OAuth 上游错误均已脱敏；
- 已移除源码内嵌的 legacy Google OAuth client ID/secret，改为显式环境配置；
- 已移除 OAuth 成功日志中的 access token 前缀；
- Codex browser PKCE/AuthSession 安全核心已实现（S256、state、loopback URI 校验、超时、取消、替换与单次消费）；独立 callback listener、token exchange/refresh、Secret Store 持久化和 schema v2 迁移尚未实现。

供应商侧旧 OAuth secret 的轮换/撤销属于外部操作，不能仅通过代码提交完成。

## 1. 背景与目标

当前“账号”模型起源于 Antigravity/Google Gemini：账号必须有邮箱、Google OAuth access/refresh token、project id、设备画像和 Gemini 配额。`TokenManager` 也以这组字段作为加载、刷新、选择和持久化的基本单位。

这与下一阶段产品方向不匹配：

- 产品主路径需要转向 Codex/OpenAI；
- 用户可能通过 API Key、官方登录流程、外部凭据引用或云供应商凭据接入；
- 一个身份可以有多份凭据，一份凭据可以建立多个连接；
- 客户端协议、上游协议、供应商和认证方式不是同一个维度；
- Gemini/Antigravity 仍需兼容迁移，但不再定义核心模型和默认体验。

本需求的目标是把账号管理升级为一个 **Codex 优先、供应商中立、凭据安全、能力驱动** 的连接平台。Codex 优先指默认产品体验和首要验证路径以 OpenAI/Codex Responses 为中心，不表示把领域模型再次绑定到单一供应商或协议。

## 2. 已确认需求与设计假设

### 2.1 已确认

1. Codex/OpenAI 成为账号管理与代理使用的主要场景。
2. 支持多种账号认证方式，而不是只支持 Google OAuth refresh token。
3. Gemini 协议和 Antigravity 账号降级为兼容路径，不再是核心账号模型。
4. 账号系统需要容纳更多上游协议，并允许协议继续扩展。
5. 旧数据必须有明确迁移与回滚策略，不能静默丢失。

### 2.2 已验证的 Codex 登录基线

1. 首期支持 OpenAI Platform API Key，并以 Responses API 作为首要上游协议。
2. Codex ChatGPT 登录支持 OAuth 2.0 Authorization Code + PKCE、localhost callback 和 state 校验；无浏览器环境可使用官方 device code flow（当前为 beta）。
3. Codex 登录缓存可位于 `CODEX_HOME/auth.json` 或系统凭据库；ChatGPT 登录 token 可在使用时自动刷新。
4. `CLIProxyAPI` 已提供可运行的 PKCE、callback、device flow、token refresh、账号元数据和认证文件管理参考，可用于本项目的接口与测试设计。

### 2.3 仍需验证的实现约束

1. MyProxy Manager 是直接实现官方兼容的 OAuth adapter，还是优先委托已安装的 Codex CLI；两条路径需要用可维护性和凭据所有权 PoC 决策。
2. 导入本机 Codex 登录缓存必须经过用户明确授权，并兼容 file、keyring、auto 三种存储策略；不能假设 `auth.json` 一定存在。
3. device code flow 仍为 beta，必须具备功能开关、不可用回退和版本兼容测试。
4. Amazon Bedrock 可作为首批扩展供应商候选，以验证非 OpenAI 凭据和 Responses 协议的组合。
5. 旧 Antigravity 能力先通过 legacy adapter 保持可读和可运行，再决定长期保留范围。

## 3. 统一术语

“账号”仅作为 UI 上的用户友好称呼，内部不得继续承担所有含义。

| 概念 | 定义 | 示例 |
|---|---|---|
| Identity | 人、团队、工作区或供应商主体的非机密信息；邮箱可选 | OpenAI project、AWS profile、Google 用户 |
| Credential | 一份可用于认证的秘密或外部秘密引用 | API Key、OAuth token set、AWS profile ref |
| AuthMethod | 获取或提供 Credential 的方式 | API Key 输入、OAuth PKCE/device、CLI 委托、环境引用 |
| AuthSession | 一次交互式认证过程及状态 | pending、authorized、expired、cancelled |
| ProviderConnection | 供应商端点、身份、凭据和配置的组合 | OpenAI production、Bedrock us-east-1 |
| WireProtocol | 与上游或客户端通信的具体协议 | OpenAI Responses、Chat Completions、Anthropic Messages |
| CapabilityProfile | 连接实际支持的模型和能力集合 | streaming、tools、vision、reasoning |
| PoolMember | 可参与路由的连接/凭据组合及运行时健康状态 | enabled、degraded、rate_limited |

必须满足以下关系：

- 一个 `Identity` 可以关联零到多份 `Credential`；
- 一份 `Credential` 只能通过其声明的 `AuthMethod` 解释和刷新；
- 一个 `ProviderConnection` 引用一个 `Credential`，但不持有明文秘密；
- 一个连接可以声明多个 `WireProtocol` 和能力；
- 路由先按请求所需能力筛选，再考虑协议、策略、健康和限额；
- 邮箱、project id、refresh token 和 quota percentage 均不得成为所有连接的必填字段。

## 4. 用户场景

### 4.1 添加 OpenAI API Key 连接

用户输入 API Key，可选填写 base URL、organization/project 和标签。系统保存为秘密引用，验证凭据，探测可用协议与能力，展示脱敏连接信息，并允许该连接参与路由。

验收标准：

- UI、日志、普通导出和管理 API 都不返回原始 Key；
- 验证失败不会留下可路由的半成品连接；
- 无法探测的能力显示为 `unknown`，不能推断为支持；
- 用户可以更新、禁用、删除或重新验证连接。

### 4.2 通过官方 Codex/ChatGPT 登录方式接入

用户发起官方支持的 Codex 登录流程。桌面环境默认使用 Authorization Code + PKCE 和仅监听 loopback 的回调服务；headless 场景可选择 device code。系统显示授权状态，并将 token set 存为受保护的凭据或外部引用。

验收标准：

- 不采集 ChatGPT 密码，不抓取浏览器 Cookie，不调用私有 Web API；
- 使用高熵 PKCE verifier、S256 challenge 和一次性 state；
- callback 只绑定 loopback，支持端口占用、取消、超时、错误回调、重复回调和 state 不匹配处理；
- access、refresh、ID token 作为同一 `codex_oauth_token_set` 的秘密材料，刷新操作并发去重；
- 账号摘要可展示 email、ChatGPT account id 指纹、workspace/plan 等非秘密元数据，但不得返回原始 ID token；
- UI 能区分“登录授权已撤销”“refresh token 已失效/复用”“本地引用失效”和“设备码不可用”；
- device code 在 beta 期间由功能开关控制，并回退至浏览器登录、手工回调 URL 或 CLI 委托。

### 4.3 引用外部或云凭据

用户选择 AWS profile、环境变量或其他受支持的外部秘密来源。系统只保存引用和非敏感元数据，由 provider adapter 在运行时解析。

验收标准：

- 默认不复制外部秘密到账号 JSON；
- 引用不可用时连接进入 `unavailable`，其他连接继续工作；
- 错误信息说明缺少哪类引用，但不输出秘密值。

### 4.4 迁移旧 Antigravity 账号

升级后系统读取旧账号文件，将其映射为 `google-antigravity-legacy` provider、`google_oauth_refresh` credential 和 `gemini_v1_internal` protocol。用户可以继续使用、禁用或删除旧连接，但新建连接页面不再默认展示该方式。

验收标准：

- 首次迁移前生成可恢复备份；
- 迁移是幂等的，同一旧账号不会重复生成；
- 旧数据无法完整转换时标记为 `migration_attention`，不静默丢弃；
- 在至少一个稳定版本周期内支持只读回滚或恢复旧数据。

## 5. 功能需求

### 5.1 连接与凭据管理

- FR-001：系统应支持创建、验证、更新、禁用、删除和重新授权 ProviderConnection。
- FR-002：系统应允许同一供应商存在多条连接，并允许同一 Identity 关联多份 Credential。
- FR-003：Credential 必须采用带版本的判别联合类型，不允许继续使用全局 `access_token/refresh_token/email` 结构。
- FR-004：Provider adapter 必须声明支持的 AuthMethod、WireProtocol 和能力探测方式。
- FR-005：连接状态至少包括 `draft`、`validating`、`ready`、`degraded`、`unavailable`、`disabled` 和 `migration_attention`。
- FR-006：交互式 AuthSession 必须独立于持久连接，可取消、超时并防止重放。
- FR-007：Codex AuthAdapter 必须同时表达 browser PKCE、device code 和 existing-cache import/delegation 三种入口的能力状态。
- FR-008：同一 Codex 身份在不同 workspace/plan 下不得因邮箱相同而互相覆盖；稳定键优先使用 provider subject/account id 的不可逆指纹。
- FR-009：refresh token 更新必须原子替换完整 token set，并通过 singleflight/锁避免并发刷新导致 token reuse。

### 5.2 首期认证方式

| 优先级 | 认证方式 | 首期要求 |
|---|---|---|
| Must | OpenAI API Key | 输入或 SecretRef；支持可选 org/project/base URL |
| Must | Codex/ChatGPT browser PKCE | loopback callback + state + S256；可参考 `CLIProxyAPI` Codex authenticator |
| Should/Beta | Codex device code | headless 登录；功能开关和 browser/CLI 回退 |
| Should | Codex cache import/CLI 委托 | 显式授权；兼容 auth.json 与系统 keyring，不假设文件存在 |
| Must | Generic OpenAI-compatible API Key | 自定义 base URL、headers 白名单、能力手工覆盖 |
| Must/Legacy | Google OAuth refresh token | 只作为旧账号迁移和兼容适配器 |
| Should | AWS credential/profile/Bedrock API Key | 用于验证云凭据与 Responses provider |
| Should | Anthropic API Key | 支持原生 Messages provider |
| Could | Azure OpenAI、企业 SSO、自定义 OAuth | 在 provider/auth 插件契约稳定后加入 |

### 5.3 协议和能力

- FR-020：客户端输入协议与上游协议必须分别建模。
- FR-021：首要上游协议为 OpenAI Responses；Chat Completions 作为兼容协议。
- FR-022：系统可同时保留 Anthropic Messages、Gemini 和 Codex wire 格式，但不得用其决定 Identity 或 Credential 类型。
- FR-023：CapabilityProfile 至少表达模型、streaming、tools、vision/images、reasoning、structured output、最大上下文、已知限额及 `unknown` 状态。
- FR-024：能力来源需记录为 `discovered`、`configured` 或 `inferred`；推断能力不能覆盖显式探测结果。
- FR-025：缺失配额信息表示 `unknown`，不得按 100% 剩余处理。

### 5.4 路由与调度

- FR-030：请求先规范化为所需能力，再筛选可用 ProviderConnection。
- FR-031：候选连接必须同时满足启用状态、协议可达、凭据可用和能力要求。
- FR-032：候选排序可组合优先级、健康、限流、会话粘性、成本和用户策略；不得以邮箱作为选择键。
- FR-033：重试只能切换到兼容连接；工具、图像或 reasoning 能力不得在切换时静默降级。
- FR-034：限流与配额使用统一 `UsageSnapshot`，允许 provider-specific 扩展字段。

### 5.5 导入、导出和展示

- FR-040：账号列表的主实体改为 Connection；Identity 作为分组或元数据展示。
- FR-041：列表至少显示 provider、认证类型、端点、协议/能力、健康、最近验证时间和脱敏标识。
- FR-042：普通导出不包含秘密；带秘密备份必须显式确认、加密并提示恢复风险。
- FR-043：任何旧的 `email + refresh_token` 导出入口必须标记 legacy，且默认关闭。

## 6. 非功能与安全需求

- NFR-001：秘密不得写入日志、错误文本、事件 payload、前端状态持久化或未加密账号文件。
- NFR-002：优先使用系统钥匙串或等价 secret store；领域与数据库只保存 `SecretRef`。
- NFR-003：认证适配器必须提供统一的校验、刷新/解析、撤销能力描述和敏感字段清理接口。
- NFR-004：当前源码中嵌入的 Google OAuth client secret 必须作为 P0 事件处理：从代码和历史可达构建中移除，并在供应商侧轮换或撤销；文档不得记录秘密值。
- NFR-005：连接验证和刷新应有超时、取消、并发去重和结构化审计记录。
- NFR-006：领域错误使用稳定错误码；UI 不依赖供应商原始错误字符串判断状态。
- NFR-007：新 provider 不应要求修改核心 Identity/Credential 结构或所有协议 handler。
- NFR-008：旧数据迁移必须幂等、可观测、可恢复，并有 fixture 和失败注入测试。
- NFR-009：本地管理 API 返回的 credential DTO 必须始终脱敏，即使调用方来自 localhost。
- NFR-010：OAuth callback server 只能绑定 loopback，不得参考实现中的 `0.0.0.0` 监听方式。
- NFR-011：不得提供默认开启的原始认证文件下载接口；导入必须校验 JSON 大小、provider、字段白名单和文件权限。
- NFR-012：JWT claim 解析不得替代签名、issuer、audience、nonce/state 或 token exchange 校验；未验签 claim 只能作为不可信展示提示。
- NFR-013：文件型 Codex 缓存必须使用目录 `0700`、文件 `0600`、原子替换；默认优先系统 keyring。
- NFR-014：文件名不得直接包含完整 email、account id 或 plan；使用随机 ID/指纹，展示信息放入脱敏 DTO。

## 7. 范围划分

### 7.1 第一阶段必须交付

1. 新领域模型、schema v3 和 SecretRef 抽象。
2. OpenAI API Key provider connection。
3. Codex browser PKCE 登录、token refresh 和脱敏账号摘要。
4. OpenAI Responses 上游执行的最小纵向切片。
5. Generic OpenAI-compatible API Key 连接。
6. legacy Antigravity 数据读取、迁移和兼容 adapter。
7. 能力/健康驱动的最小连接选择器。
8. 账号页面改为连接视图，所有秘密默认脱敏。
9. device code 与 existing-cache import/CLI 委托 PoC；验证通过后按 beta/正式状态开放。

### 7.2 后续阶段

- AWS/Bedrock 与 Anthropic provider adapter；
- 多凭据轮换、成本策略和高级限额模型；
- Azure OpenAI、企业身份和自定义 OAuth；
- provider SDK/插件边界及第三方扩展；
- 加密备份与跨设备迁移。

### 7.3 明确不做

- 不抓取 ChatGPT/Gemini 浏览器 Cookie；
- 不保存用户密码；
- 不模拟未公开 OAuth client 或私有 Web API；
- 不保证所有协议间无损转换；
- 不把未知配额、模型或能力伪装成可用；
- 不在 schema v3 上线时立即删除旧账号原始数据。

## 8. 概念数据模型

```text
Identity {
  id, provider_namespace, subject?, display_name?, email?, workspace?, metadata
}

Credential {
  id, identity_id?, auth_kind, secret_ref, lifecycle_state,
  expires_at?, scopes?, fingerprint, created_at, updated_at
}

ProviderConnection {
  id, provider_kind, identity_id?, credential_id, endpoint,
  config, enabled, status, last_validated_at
}

CapabilityProfile {
  connection_id, protocols[], models[], features{}, limits{},
  source, discovered_at, expires_at?
}

UsageSnapshot {
  connection_id, model?, rpm?, tpm?, remaining_ratio?, credits?,
  reset_at?, status, provider_details{}, observed_at
}
```

Credential 的建议判别类型：

```text
openai_api_key | codex_oauth_token_set | external_cli_ref |
aws_profile_ref | bearer_token_ref | google_oauth_refresh_legacy
```

类型名称表达认证材料，不表达上游协议。所有具体秘密字段只存在于 secret store 中。

## 9. 迁移要求

旧 schema v2 到 v3 的基准映射：

| 旧字段/概念 | 新对象 |
|---|---|
| `account.id/email/name` | Identity + display metadata |
| `token.access_token/refresh_token` | `google_oauth_refresh_legacy` Credential 的 SecretRef |
| `project_id` | legacy ProviderConnection config |
| `device_profile/device_history` | legacy adapter 私有 metadata |
| `quota` | provider-specific UsageSnapshot |
| enabled/proxy/label | ProviderConnection 与 PoolMember 配置 |

迁移顺序：

1. 启动时只读检测旧 schema，不原地覆盖；
2. 创建带时间戳的备份和 migration journal；
3. 将秘密先写入 secret store，再写 v3 引用；
4. 校验数量、指纹和必要字段；
5. 原子切换 active schema；
6. 失败时回退到旧读取路径，并展示可操作错误；
7. 稳定期结束后再单独决策是否清理旧秘密文件。

## 10. 验收与质量门槛

第一阶段完成必须同时满足：

- OpenAI API Key 从创建、验证、能力发现到 Responses 请求形成端到端闭环；
- Codex browser PKCE 从创建 AuthSession、callback、token exchange、SecretRef 持久化到刷新形成端到端闭环；
- state mismatch、回调重复、超时、端口占用、refresh token reuse 和刷新并发均有自动化测试；
- 任何管理 API、日志、导出和前端快照均无法检索到测试秘密；
- 至少两种 credential kind 和两种 provider kind 通过同一核心连接用例；
- legacy fixture 可幂等迁移，故障注入后可恢复；
- 路由测试证明不兼容能力不会被选中或静默降级；
- Desktop/Tauri 与 HTTP 管理入口共享 application use case 和 DTO；
- schema、adapter、迁移和安全行为均有 Rust contract/integration tests；
- 账号页面覆盖成功、无数据、验证失败、凭据失效和迁移待处理状态。

## 11. 待决策项与 Spike

| 项目 | 要回答的问题 | 退出条件 |
|---|---|---|
| Codex adapter 路径 | 原生 browser PKCE 与 Codex CLI 委托如何分工？ | 两种 PoC 的凭据所有权、刷新、登出和版本兼容对比 |
| Codex cache import | 如何同时兼容 auth.json、keyring 与 CODEX_HOME？ | 显式授权、只读探测、导入/引用和登出边界测试 |
| Device code beta | 不可用或协议变化时如何回退？ | 功能开关 + browser/CLI 回退 + 兼容性测试 |
| Secret store | 各桌面平台使用系统钥匙串还是统一加密 vault？ | macOS/Windows/Linux 可用性与恢复测试 |
| Bedrock 首期范围 | 使用 AWS SDK credential chain 还是仅 profile/ref？ | Responses 请求与错误/区域测试 |
| 能力发现 | 哪些 provider 可探测，哪些需静态声明？ | 形成 provider capability contract |
| legacy 生命周期 | Antigravity 新增入口何时移除？ | 使用数据、迁移成功率和发布说明评审 |

上述 Spike 不再阻塞 browser PKCE 主路径，但 device code、缓存复用和 CLI 委托在完成退出条件前只能标记为实验能力；核心 schema 只固化 OAuth token set 与 SecretRef，不固化第三方参考项目的文件格式。

## 12. 官方依据

- [Codex authentication](https://developers.openai.com/codex/auth/)：确认 ChatGPT/API Key 登录、登录状态检查、`auth.json`/keyring/auto 存储、自动刷新、device code、localhost callback 和缓存安全要求。
- [OpenAI API quickstart](https://platform.openai.com/docs/quickstart/make-your-first-api-request)：确认 API Key 是 OpenAI Platform 的正式接入方式。

官方材料确认了 Codex 的登录和缓存基线，但没有承诺 `CLIProxyAPI` 的内部常量、JSON 字段或管理 API 是稳定公共接口；本项目必须通过 adapter 隔离其变化。

## 13. `CLIProxyAPI` 参考实现评估

参考仓库：`/Users/Jacky/workspace/AIWorks/dc_works/todo/CLIProxyAPI`。

### 13.1 可直接借鉴的设计

| 参考位置 | 可借鉴内容 | MyProxy Manager 对应设计 |
|---|---|---|
| `internal/auth/codex/pkce.go` | 高熵 verifier、S256 challenge | `CodexPkceService` |
| `internal/auth/codex/openai_auth.go` | auth URL、code exchange、refresh、singleflight | `CodexOAuthAdapter`，端点和字段封装在 adapter 内 |
| `internal/auth/codex/oauth_server.go` | callback 结果、超时与关闭生命周期 | loopback-only `AuthCallbackServer` |
| `sdk/auth/codex.go`、`codex_device.go` | browser/device 两种入口、手工 callback、统一 Auth record | `StartAuthSession` 用例与 flow strategy |
| `internal/api/handlers/management/oauth_sessions.go` | pending/completed/error/cancelled 会话与 TTL | 持久连接之外的短生命周期 AuthSession |
| `internal/auth/codex/token.go` | token set、账号元数据与刷新时间 | secret payload + 脱敏 CredentialSummary |
| `sdk/cliproxy/auth/types.go` | 状态、错误、quota、per-model availability | ConnectionHealth、UsageSnapshot、ModelState |
| `internal/runtime/executor/codex_executor.go` | Bearer token、account header、刷新和错误分类 | Codex ProviderAdapter |

### 13.2 不直接复制的部分

- 不以明文 JSON 作为默认凭据仓库；优先 keyring，文件仅作为兼容/显式备份。
- 不提供直接返回 access/refresh/ID token 的普通下载接口。
- callback 不监听所有网卡，只监听 `127.0.0.1`/`::1`。
- 不用包含完整 email/plan 的文件名作为身份主键。
- 不把未验签 JWT payload 当作可信授权依据。
- 不把 `type + map[string]any` 作为最终领域模型；provider-specific 数据留在 adapter payload，核心使用判别类型和 SecretRef。

因此，`CLIProxyAPI` 将 Codex 登录从“可行性未知”提升为“已有参考实现、需要安全加固和 Rust 化验证”。它是实现样本，不是协议稳定性的唯一依据。
