import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/exterior.ts';let s=fs.readFileSync(p,'utf8');s="import { buildApartmentPodium } from './apartmentPodium';\n"+s;
const a=s.indexOf('  /* ================= 底层：入口 + 生活设施'),b=s.indexOf('  // 窗玻璃收尾合并',a);if(a<0||b<0)throw Error('replacement markers missing');s=s.slice(0,a)+`  // Replace the entire ground-floor frontage and its clutter with a residential podium.
  const podium = buildApartmentPodium();
  g.add(podium.group);
  boxes.push(...podium.boxes);

`+s.slice(b);
s=s.replace("  put(gbox(W, F2 - GAP, DEPTHB2), M.wallBase, 0, (F2 - GAP) / 2, CZB2).name = 'apt-ground';",`  // Actual voids behind the new glazing: 2.4 m wings and a 4.3 m central lobby.
  for(const [left,right,front] of [[X0,-6.2,2.2],[-6.2,6.2,.22],[6.2,X1,2.2]]) {
    const back=ZN+ROOM_D;
    put(gbox(right-left,F2-GAP,front-back),M.wallBase,(left+right)/2,(F2-GAP)/2,(front+back)/2).name='apt-ground';
  }`);
s=s.replace("APT_X0 - SHELL_T, -1.0, 0, FLOORS[0], APT_ZF", "APT_X0 - SHELL_T, -1.2, 0, FLOORS[0], APT_ZF").replace("'apt-south-1f-e', 1.0,", "'apt-south-1f-e', 1.2,");
const ca=s.indexOf('  /* —— 1F 入口门厅：'),cb=s.indexOf('  /* —— 北立面',ca);if(ca<0||cb<0)throw Error('collision markers missing');s=s.slice(0,ca)+`  // Ground-floor lobby walls and furniture are supplied by buildApartmentPodium.

`+s.slice(cb);fs.writeFileSync(p,s);
const ap='plugins/3d-apartment/src/anime/apartmentArchitecture.ts';s=fs.readFileSync(ap,'utf8');const aa=s.indexOf('  // Recessed entrance canopy'),ab=s.indexOf('  // Materials are private',aa);s=s.slice(0,aa)+s.slice(ab);fs.writeFileSync(ap,s);
