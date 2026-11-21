# 3D 公寓插件

独立目录包含 `src/` 场景、导航和物理逻辑、`room/` 模型、`tools/` 开发与验证脚本、`art/` Blender 工程，以及本插件自己的 npm 依赖清单。`art/` 和生成的 `ui/` 不进入版本管理；发行包只包括清单、构建的脚本和运行时 GLB。

在仓库根运行：

```powershell
npm install
npm run build:apartment
npm run package:small
```

`build:apartment` 单独类型检查和构建插件，生成 `release/Vivian-3D-Apartment-1.1.0.zip`。`package:small` 生成基础 NSIS 安装程序与配套 ZIP。主程序的 `npm run build` 不构建也不打包本插件。

发行时把 ZIP 和 setup 放在同一目录。Setup 可以选择安装公寓；没有 ZIP 时该选项不可选。已安装后可在设置 → 通用启用或禁用。静默安装默认仅安装基础程序，使用 `/APARTMENT` 安装配套组件；`/NOAPARTMENT` 优先于它。卸载会删除公寓插件。

安装时验证 ZIP 的 SHA256、允许的文件路径和清单，再解压到临时目录并替换已安装插件，失败恢复旧插件。可直接将 ZIP 的内容放到程序目录的 `plugins/3d-apartment/` 下。

主程序仅保留必须由宿主实现的接口：插件安装状态与设置、窗口创建、桌宠隐藏/恢复、Windows ESC 看护，以及通用世界快照命令。宿主不包含 Three.js、场景、美术资源或导航逻辑。插件通过 `src/hostContract.ts` 声明所需世界快照片段，不导入主程序源码。

开发时运行仓库根的 `npm run dev`，预览地址 `/plugins/3d-apartment/preview.html`。模型从插件目录读取。
