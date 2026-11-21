import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/apartmentLift.ts';let s=fs.readFileSync(p,'utf8').replace("for(const x of [-35.90,-33.30])box('cabin-front',x,F+1.40,-.90,.50", "for(const x of [-36.10,-33.10])box('cabin-front',x,F+1.40,-.90,1.10").replace("F+1.25,-.81,.95", "F+1.25,-.92,.95");fs.writeFileSync(p,s);
const ap='plugins/3d-apartment/src/anime/apartmentPodium.ts';s=fs.readFileSync(ap,'utf8').replace('group.userData.podiumVersion=2','group.userData.podiumVersion=3');fs.writeFileSync(ap,s);
const check='plugins/3d-apartment/tools/room/check-lift.mjs';s=fs.readFileSync(check,'utf8');s=s.replace('lift.dispose();',`const pairs=[];let stamp=50000;
 for(let from=0;from<4;from++)for(let to=0;to<4;to++){
   fps.setPosition(-34.6,[0,3.4,6.2,9][from]+1.662,-2.4);lift.update(stamp,true);
   const accepted=lift.selectFloor(to,stamp);if(accepted!==(from!==to))throw Error('Invalid same-floor selection');
   if(accepted){lift.update(stamp+liftTravelMs(from,to),true);if(Math.abs(fps.getPosition().y-([0,3.4,6.2,9][to]+1.662))>.001)throw Error('Wrong floor pair');pairs.push([from+1,to+1]);}
   stamp+=10000;
 }
 fps.setPosition(0,1.662,0);lift.update(stamp,true);if(lift.selectFloor(2,stamp))throw Error('Outside cabin accepted');
 lift.dispose();`);s=s.replace('return{route,arrived,returned,','return{route,arrived,returned,pairs,');fs.writeFileSync(check,s);
