import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/apartmentPodium.ts';let s=fs.readFileSync(p,'utf8');
const cut=(a,b)=>{const i=s.indexOf(a),j=s.indexOf(b,i);if(i<0||j<0)throw Error(a);s=s.slice(0,i)+s.slice(j);};
cut("    box('wing-floor'","    // 12 mm masonry");cut('    // Oak privacy screen',"    sconce(x+5.17");
const i=s.indexOf("  box('lobby-floor'"),j=s.indexOf("  box('entrance-header'",i);s=s.slice(0,i)+`  // A single continuous ground floor, independent of the five apartments above.
  box('lobby-floor',0,.037,-.55,62,.05,10.54,stone,true);
  box('lobby-ceiling',0,3.265,-.55,61.98,.07,10.48,plaster);
`+s.slice(j);
cut("  box('elevator-surround'",'  for(let col=0;col<6;col++)');
s=s.replace("box('mail-front',x,y,.48", "box('mail-front',x,y,-5.58").replace("box('mail-slot',x,y+.09,.565", "box('mail-slot',x,y+.09,-5.495").replace("-4.12,2.47,.425", "-4.12,2.47,-5.635");
s=s.replace("    box('rear-cavity-back',x,1.64,-4.98,12.32,3.14,.06,dark);",'');
s=s.replace("  for(const s of [-1,1]){\n    box('end-plinth'", "  for(const s of [1]){\n    box('end-plinth'");
// Account for CRLF in this pre-existing file.
s=s.replace("  for(const s of [-1,1]){\r\n    box('end-plinth'", "  for(const s of [1]){\r\n    box('end-plinth'");
const end=s.indexOf('  group.userData.podiumVersion');s=s.slice(0,end)+`  // Slender structural columns preserve a broad, continuous east-west route at z=0.
  for(const x of [-24,-12,12,24])for(const z of [-3.9,3.6])box('lobby-column',x,1.65,z,.26,3.15,.26,stone,true,true);
  // West coffee bar: a working counter, back bar, espresso machine and open seating.
  box('cafe-counter',-14,.58,-3.08,8.2,1.05,.88,wood,true,true);
  box('cafe-stone-top',-14,1.135,-3.08,8.34,.06,1.02,stone);
  box('cafe-back-cabinet',-14,.52,-5.21,8.2,.91,.61,wood,true);
  box('cafe-back-top',-14,1.01,-5.21,8.3,.045,.68,stone);
  for(const y of [1.65,2.25]){
    box('cafe-shelf',-14,y,-5.40,7.8,.045,.37,wood);
    for(let n=0;n<14;n++)box('cafe-canister',-17.45+n*.53,y+.13,-5.42,.15,.21,.16,n%3===0?bronze:linen);
  }
  label('V I V I A N   C O F F E E',-14,2.80,-5.62,4.4,.28);
  box('espresso-machine',-15.8,1.38,-3.18,1.12,.43,.49,bronze,false,true);
  box('espresso-face',-15.8,1.38,-2.925,.98,.29,.025,dark);
  for(const x of [-16.1,-15.55]){
    box('espresso-spout',x,1.24,-2.85,.045,.12,.13,bronze);
    box('coffee-cup',x,1.21,-2.77,.11,.10,.11,linen,false,true);
  }
  box('pastry-case',-12,1.37,-3.05,1.6,.39,.53,bronze);
  box('pastry-display',-12,1.38,-2.772,1.48,.28,.016,linen);
  for(const x of [-17,-14,-11]){
    box('barstool-base',x,.39,-1.95,.13,.66,.13,bronze,true);
    box('barstool-seat',x,.75,-1.95,.43,.075,.43,sage,false,true);
    box('cafe-pendant',x,2.70,-3.0,.43,.16,.43,warm,false,true);
    box('pendant-wire',x,2.99,-3.0,.012,.42,.012,bronze);
  }
  for(const x of [-22,-16,-10]){
    box('cafe-table-leg',x,.36,1.65,.14,.62,.14,bronze,true);
    box('cafe-table',x,.72,1.65,1.20,.065,.85,stone,true,true);
    seat(x,2.7,1.7,linen,true);
  }
  // East lounge and shared reading table; no enclosing partitions.
  for(const x of [11,18,25]){
    seat(x,2.95,3.1,sage,true);
    box('lounge-table',x,.37,1.65,1.4,.10,.7,wood,true,true);
    box('lounge-table-foot',x,.19,1.65,.72,.25,.43,dark);
  }
  box('library-table',18,.76,-2.9,7.8,.075,1.15,wood,true,true);
  for(const x of [14.6,21.4])box('library-leg',x,.39,-2.9,.13,.68,.9,bronze,true);
  for(const x of [15,17,19,21])seat(x,-4.05,.72,linen,true);
  for(let n=0;n<5;n++){
    box('bookcase',12+n*3.1,1.27,-5.45,2.8,2.38,.43,wood,true);
    for(let row=0;row<4;row++)for(let book=0;book<9;book++)box('book-spine',10.85+n*3.1+book*.27,.42+row*.5,-5.22,.15,.30+(book%3)*.035,.025,book%2?sage:linen);
  }
  for(const x of [-21,-12,12,21])box('hall-light',x,3.205,0,7,.018,.085,warm);
  label('←  LIFT  /  1—4',-28,2.65,4.435,2.7,.22,true);
`+s.slice(end);fs.writeFileSync(p,s);
const ex='plugins/3d-apartment/src/anime/exterior.ts';s=fs.readFileSync(ex,'utf8');const a=s.indexOf('  // Actual voids behind'),b=s.indexOf('  // 2 层：203',a);if(a<0||b<0)throw Error('core markers');s=s.slice(0,a)+'  // Ground floor is an open lobby; no solid apartment-core infill.\n\n'+s.slice(b);
s=s.replace('    railRun(X0 + 0.04, F, ZN - 1.22, X0 + 0.04, F, ZN + 0.02);','    // West corridor end is open to the new lift tower.');
s=s.replace(/  boxSpec\('apt-west-1f', [^\r\n]+\r?\n/, '  // Ground-floor west wall opens into the added lift hall.\n');fs.writeFileSync(ex,s);
