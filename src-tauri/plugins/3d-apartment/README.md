# 3D 公寓插件

Setup（NSIS）安装页可选择是否安装。插件安装到程序目录的
`plugins/3d-apartment/`，其中 `room/` 存放运行时模型资源，`ui/room.js`
存放完整的场景代码与前端依赖。未安装时这两部分均从安装目录移除。

安装后在「设置 → 通用」控制启用状态。关闭时，已打开的公寓窗口会退出，
入口和全局快捷键会停用；重新启用即可恢复。未安装时需重新运行 Setup 并勾选组件。

`npm run build` 分别构建插件脚本与主程序。主程序仅保留安装状态检查、入口
和启动 loading 层；公寓代码由 `vite.apartment.config.ts` 独立打包，
运行时通过 Tauri asset 协议从插件目录载入。模型 URL 由
`src/components/room/apartmentAssets.ts` 解析到插件资源目录。
