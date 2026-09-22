# Room furniture artwork

The existing apartment layout, doors, navigation and character behavior remain in
`dormLayout.json` and `RoomScene.tsx`. Forty static furniture items use the local
Blender asset `public/room/models/vivian-furniture.glb`. Original builders remain
available immediately during loading, on asset failure, and when a layout item's
kind or dimensions no longer match `blenderFurnitureManifest.json`.

The furniture includes PBR materials, embedded textures, short-range vertex AO,
rounded bedding, upholstery seams, and two authored retro computer desks.
Furniture is batched per item and material; it is never merged across rooms.
The original furniture lights remain in Three.js. A generated, low-intensity
prefiltered environment supplies soft PBR reflections without downloading an HDRI.
Its intensity follows time and weather. AO contains no baked directional sunlight.

## Local editable source

- `blender/room/vivian-furniture.blend`: editable Blender source, packed textures.
- `blender/room/procedural-base.glb`: migration base from the original builders.
- `scripts/room/build_blender.py`: geometry refinement, batching and vertex AO.
- `scripts/room/author_details.py`: authored desk props and bedding refinement.
- `scripts/room/export_blender.py`: Blender source → self-contained runtime GLB.
- `scripts/room/verify-room.mjs`: browser screenshots, fallback, light-count and
  horizontal-footprint checks at identical camera positions.

The repository's existing ignore rules deliberately exclude `blender/`, `scripts/`
and local preview entries. These source files are retained locally, but are not
versioned or included in the application package. Back up the `.blend` when moving
the artwork to another machine; the shipped GLB alone is not the editable source.

## Rebuild

With Blender 4.2 or a compatible `bpy` installation:

```powershell
blender -b --python scripts/room/build_blender.py
```

For hand-edited artwork, publish the saved source without rebuilding from the
migration base:

```powershell
blender -b blender/room/vivian-furniture.blend --python scripts/room/export_blender.py
```

Only re-run `export-base.mjs` when deliberately refreshing the migration baseline;
it requires the local Vite server and Playwright. Local preview entries are
`room-preview.html` and `src/roomPreview.tsx`. Both are intentionally ignored.

```powershell
npm run dev -- --host 127.0.0.1
node scripts/room/verify-room.mjs
npm run build
```

Set `PLAYWRIGHT_PATH` to an installed Playwright package directory if it is not
available in normal Node resolution. The verification uses locally installed
Chrome. Its intentionally aborted asset request checks the procedural fallback;
only that branch is expected to log a failed request.

The export checks GLB magic/length, embedded images and buffers, vertex AO, and a
16 MiB asset budget. The build caps furniture at 110,000 triangles. Browser reports
are in `blender/room/verification/report.json`; full-scene draw counts include
the surrounding city and characters and are not furniture-only measurements.

Visual references: the local `lofi-room_by_Astra` project for soft manufactured
forms and desktop detail; `gpt6-Astra_3.js/雨夜便利店.html` for warm interior light
against a cool rainy exterior. No reference geometry or image assets are shipped.

## 街区环境重制

`anime/districtArt.ts` 负责独立的城市美术层：分层楼群、退台与窗光、天空云层、随天气变化的雾、便利店屋顶与公寓屋顶绿化。旧远景楼群、环形贴图、商场和高架不再在 RoomScene 中装配。近景沿用交互结构，转换为标准物理材质，并为适合的箱体添加圆角；家具继续使用 Blender GLB。

美术层使用 `sceneCollideSkip`，不扩大行走碰撞范围。静态装饰按材质合批；天空每帧跟随相机，天气切换更新窗光和路面粗糙度。新增材质与纹理随场景卸载释放。主阴影覆盖街道和公寓，分辨率 2048；这会增加阴影成本，尚未在用户显卡上测量帧率。

本地验证：`scripts/room/verify-district.mjs` 输出夜景全景、晴天全景、街角和黄昏截图；`scripts/room/check-store-contracts.mjs` 验证货架碰撞、自动门与天气反射。预览仍使用根目录 `room-preview.html`。

## 公寓建筑改造与深度冲突

`anime/apartmentArchitecture.ts` 替换原阳台栏杆、通长屋顶、山墙和入口雨棚，重建分段女儿墙、屋顶设备、种植槽、陶土山墙和木格栅。房间洞口、楼层标高、外廊和折返楼梯的行走尺寸继续使用原建筑契约，76 个附属碰撞盒保持不变。203 阳台栏杆在 `props.ts` 中同步调整。

避免深度冲突使用实际几何间距：203 外墙覆板外表面比房间外墙外移 25mm；阳台饰板距结构板前缘至少 35mm；屋顶分段留 60mm 缝；山墙横饰条与立柱有不同的外表面和端部标高。旧构件直接移除，不采用两套外皮重叠或关闭深度测试。`districtArt.prepare` 跳过已经圆角的几何，避免再次按 BoxGeometry 默认参数错误重建尺寸。

本地 `scripts/room/check-apartment-surfaces.mjs` 扫描新增轴对齐箱体与其他箱体同向表面的共面交叠，并检查五段屋顶和碰撞盒数量。该检查不覆盖透明面、任意斜面或所有 GPU 深度精度情况，因此仍配合全景、阳台、山墙及背面楼梯的浏览器截图复核。

## 203 室内重设计

布局由 `dormLayout.json` 驱动：扩大主卧，重排两间卧室的床、衣柜和工作区；客厅改成南北向沙发与独立电视背景墙；厨房沿北墙布置，餐区移到南窗，东侧留入口通道。外部门窗保持与公寓立面对齐，角色出生点与十二个功能热点同步更新。

`anime/interiorDesign.ts` 管理实例独占的浅橡木、灰泥、石材和织物贴图及 PBR 材质。材质转换同时用于程序化回退家具与加载后的 Blender 家具，不修改城市共用材质。现在加载 34 件 Blender 家具；6 张地毯改为 24mm 厚的单体，不再使用两张相隔数毫米的平面。

墙面直接替换材质；踢脚线离墙 9mm；电视格栅及厨房吊柜背面离墙 30mm；墙角封口柱端部与地面/天花错开 10mm。没有新增叠加墙皮。所有新材质、纹理由室内设计实例在卸载时释放。

验证脚本：`check-interior.mjs` 检查十二个热点可达、地面无交叠和参与导航的家具占地无交叉；`check-interior-surfaces.mjs` 检查壳体平面的共面交叠；`verify-interior.mjs` 输出客厅、厨房及两间卧室的昼夜截图。平面扫描不替代斜面、透明材质与不同 GPU 下的实际检查。截图保存在 `blender/room/interior-verification/`。

## 街区规划与道路材质

`anime/urbanStreets.ts` 是道路及地块的共同数据源。约 160×161m 的场景中，六条东西道路与两条南北集散道路形成连通路网；公寓—便利店保留慢行商业主街，36 块新增住宅/商住用地按道路和人行道退界布置。新增建筑门口用步道连接最近街道，南侧社区绿地连接前后两条街道。新建建筑只提供外观与实体碰撞，不包含可进入室内。

街道尺度按场景安排：商业主街保留既有 5m 车行断面，其余横向道路 6.6m，纵向道路 8m，新街坊步道 2.4m；旧公寓、店铺与地铁入口保留原有接口宽度。树池占设施带并留出通行空间。路口配置斑马线和停止线，沿路布置排水口、路缘石与街灯。

沥青使用以 2m 为重复尺度的颜色、粗糙度及微凹凸贴图，线性数据贴图不使用 sRGB。雨天与晴天切换粗糙度；商店既有低分辨率平面反射继续使用。所有旧沥青和广场铺装在装配时换用相同材质及世界坐标 UV。路口由东西道路统一占有，纵向道路在交界处分段，避免重复路面；步道和标线分开标高。建筑、树池和长椅增加碰撞，观察范围和地面支撑范围同步扩大。

布局思路参考国土交通省街道设计与步行空间资料：https://www.mlit.go.jp/toshi/walkable/guideline/ 。宽度是本场景的设计参数。

本地检查：`check-urban-plan.mjs` 检查建筑与道路/步道退界、平面共面交叠以及晴雨材质切换；`verify-urban.mjs` 输出总览、路口、路面近景、雨夜和绿地截图，位于 `blender/room/store-verification/urban-*.png`。

## Residential podium redesign

`anime/apartmentPodium.ts` replaces the complete original ground-floor frontage, storefronts, exposed service cabinets, bicycle canopy, refuse enclosure and entrance planting. The new design uses honed limestone courses with physical joints, bronze-framed glazing, oak screens and canopy soffits, sheltered planting, and integrated wayfinding. The central lobby is 4.3 m deep with a 2.4 m clear entrance, mail storage, concierge and seating; wings have 2.4 m deep visual amenity spaces. Rear elevations use a consistent residential service/amenity frontage.

`exterior.ts` cuts corresponding voids out of the ground-floor mass and uses the podium's declarative colliders instead of the former shallow-vestibule barriers. Upper apartments and external stair routes are retained. Entry door leaves are modeled in their open position; the elevator is decorative. No additional dynamic door mechanism is introduced.

Verification: `check-podium.mjs` checks podium/core coplanar box faces and entry clearance; `check-podium-walk.mjs` exercises the real FPS controller through entry, back-wall stop and exit; `verify-podium.mjs` captures front, wing, lobby, rear and night views. Materials and textures are owned by the scene and disposed by its existing teardown.

### Rear elevation correction
The rear shallow display bays are replaced by solid limestone masonry, four high privacy windows, closed timber resident doors and louvered service doors. Oversized amenity labels and display seating are removed. A continuous level paved path and planting pockets define pedestrian access; a small sign directs residents to the existing eastern stair. Rear doors are currently closed architectural elements, with the north wall collision retained. The accessible main lobby remains on the south side.

## Continuous lobby and operational lift

The ground-floor mass and five internal room partitions are removed. One continuous stone floor and ceiling connect the coffee bar in the west, concierge/mail area in the centre, and lounge/library in the east. Structural columns and furniture use explicit collision boxes; the east-west circulation route at z=0 stays clear.

`anime/apartmentLift.ts` adds a 5.8 m western lift tower, extending the apartment to x=-36.8. Four landing floors at Y=0/3.4/6.2/9.0 connect to the existing north corridors; the former west corridor end railing and ground-floor west barrier are removed. The 203 plan and eastern stairs remain in place.

Enter the lift cabin and press 1–4 (top row or numpad). Travel lasts 1200 + 1500 × crossed-floor-count milliseconds. During travel, walking is suspended and door leaves close; arrival teleports the FPS camera to the destination cabin on the world's vertical Y axis, clears held movement/momentum and reopens the doors. This intentionally models timed transfer rather than a physically moving cabin. Leaving first-person mode cancels the trip. The overlay/listener is removed on scene cleanup.

Checks: `check-lift.mjs` verifies the continuous lobby route, upper-corridor access, all 12 distinct floor pairs, no early transfer, duplicate/same-floor/outside-cabin rejection and cancellation. `check-lift-surfaces.mjs` checks box-face overlaps against the original shell. `verify-lift.mjs` exercises real pointer lock and keyboard floor selection in the running preview, including before/after arrival screenshots.
