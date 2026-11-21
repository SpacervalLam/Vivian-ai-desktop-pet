# 多来源天气

天气感知继续使用 Open-Meteo，并可在「设置 → 真实世界感知／世界与天气」中启用 Apple WeatherKit。

## 常驻感知

每个来源独立提供当前气温、湿度、海平面气压、今日最高／最低温与温差，以及未来 12 小时的降水概率摘要。空气质量另列来源、时间和标准：Open-Meteo / CAMS 的 US AQI、PM2.5、PM10 是模型估计，不能当作中国站点实测 AQI。

世界快照只保存摘要，不保存逐小时或十日数组。聊天、内心独白与思绪使用相同摘要；缓存沿用天气 TTL。位置、Apple 凭据或开关改变会清空缓存，并丢弃配置改变前尚未完成的请求结果。单个来源失败不影响其他来源。

## 按需预报与比对

`get_weather_forecast` 参数：

- `days`：1–16，默认 10，包含今天。Apple 最多提供 10 天。
- `hours`：1–240，默认 24，从当前小时起。
- `source`：`all`（默认）、`open_meteo`、`apple`。

返回 `providers` 中各来源的当前天气、逐小时与每日预报，及单独的 `air_quality`、`source_status`。缺失数值保持 `null`。同地点、同时间、同单位的数据才适合比较，不自动平均各来源。Apple 返回 RFC3339 时刻；其 `timezone` 指每日预报聚合所用时区，UTC 日期部分可能与当地日期不同。气温为 °C、湿度与概率为 %、气压为 hPa、风速为 km/h、降水量为 mm。

## 角色的天气关心

共享对话与主动陪伴提示词要求结合真实天气、地点、出门和返程时段，针对降雨、酷热、严寒、大温差、空气污染及雷暴给出一两条实用建议。普通降雨不当成紧急危险，预报概率不当成确定事件，日最高温不当成当前温度；摘要未覆盖行程时段时按需查询预报。多来源分歧不取平均，也不据此声称有官方预警。

Vivian 和 Nana 各有 14 条动态语料，包含风险关心与已准备、取消、拒绝、缺失、过期和来源冲突等边界；静态中英日示例也展示提醒后收住。示例里的天气与行程明确为虚构，不能作为当前事实。主动关心保留现有近期用户原话证据、静默和冷却限制，不因天气提示词绕过调度条件。

`node tests/prompt-scenarios.test.mjs` 检查提示词约束、场景和语料结构；离线检查不代表真实模型表现。实时评测使用 `scripts/evaluate-prompts.mjs --live`，需配置评测模型凭据。

## Apple 配置

需要自己的 Apple Developer Program 会员与 WeatherKit 权限。填写 Team ID、Service ID、Key ID，粘贴完整的 PKCS#8 PEM `.p8` 私钥。输入框支持直接粘贴多行内容，也可显示并编辑。私钥使用已有配置秘密存储（Windows DPAPI 当前用户加密），调试输出隐藏私钥；生成的一小时 ES256 JWT 只用于请求，不注入角色上下文。

可选 IANA 时区覆盖如 `Asia/Shanghai`。留空优先使用 Open-Meteo 按坐标返回的时区；不可用时使用系统 IANA 时区，系统解析失败时使用 `Etc/UTC`。查询 `source=apple` 时不会请求 Open-Meteo，因此异地位置应显式配置 Apple 时区。

Apple 关闭或未填写有效凭据时，Open-Meteo 可继续工作。WeatherKit 不提供此处的 AQI，空气质量仍使用单独接口。提供商数据包含归因链接，展示 Apple 数据时保留 Apple Weather 来源与法律说明。

官方文档：

- [Open-Meteo 天气](https://open-meteo.com/en/docs)
- [Open-Meteo 空气质量](https://open-meteo.com/en/docs/air-quality-api)
- [Apple WeatherKit](https://developer.apple.com/cn/weatherkit/)
- [WeatherKit REST 鉴权](https://developer.apple.com/documentation/weatherkitrestapi/request-authentication-for-weatherkit-rest-api)
