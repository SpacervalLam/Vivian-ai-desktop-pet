import fs from 'node:fs';const p='plugins/3d-apartment/src/anime/apartmentPodium.ts';let s=fs.readFileSync(p,'utf8');const a=s.indexOf('  // Rear amenities replace'),b=s.indexOf('  // Basalt base',a);if(a<0||b<0)throw Error('markers');s=s.slice(0,a)+`  // Quiet rear elevation: solid masonry, high privacy windows and closed service doors.
  // These shallow cavities are not furnished or advertised as full amenity rooms.
  const privacy=mat('reeded privacy glass','#91a49b',.56,.08);
  const paving=mat('rear sandstone pavers','#b6b1a2',.94);
  function rearWall(x0:number,x1:number,y0:number,y1:number){
    const cols=Math.ceil((x1-x0)/1.55),rows=Math.ceil((y1-y0)/.72);
    const w=(x1-x0)/cols,h=(y1-y0)/rows;
    for(let c=0;c<cols;c++)for(let r=0;r<rows;r++)box('rear-masonry',x0+(c+.5)*w,y0+(r+.5)*h,-5.88,w-.012,h-.012,.20,stone);
  }
  for(let unit=0;unit<5;unit++){
    const x=-24.8+unit*12.4,left=x-6.19,right=x+6.19;
    box('rear-cavity-back',x,1.64,-4.98,12.32,3.14,.06,dark);
    // Solid jambs and lintels are built around actual openings, never overlaid.
    if(unit===0||unit===2||unit===4){
      const half=unit===2?1.30:.76;
      rearWall(left,x-half,.29,3.18);rearWall(x+half,right,.29,3.18);
      rearWall(x-half,x+half,2.66,3.18);
      for(const dx of [-half+.033,half-.033])box('rear-door-jamb',x+dx,1.35,-5.805,.066,2.60,.15,bronze);
      box('rear-door-head',x,2.62,-5.805,half*2-.132,.06,.15,bronze);
      box('rear-threshold',x,.055,-5.76,half*2-.132,.04,.39,dark);
      for(const sign of [-1,1]){
        const leafW=half-.085;
        box('rear-closed-door',x+sign*(half/2-.028),1.35,-5.705,leafW,2.48,.07,unit===2?wood:bronze);
        box('rear-door-handle',x+sign*.14,1.23,-5.777,.028,.34,.045,bronze);
        if(unit!==2)for(let n=0;n<12;n++)box('rear-door-louver',x+sign*(half/2-.028),.50+n*.105,-5.755,leafW-.10,.035,.025,dark);
      }
      sconce(x-half-.42,-6.045,true);
      if(unit===2){
        box('rear-entry-canopy',x,2.93,-6.32,3.42,.105,1.06,bronze);
        box('rear-entry-soffit',x,2.857,-6.32,3.22,.028,.92,wood);
        box('rear-entry-light',x,2.827,-6.37,2.7,.012,.045,warm);
        box('rear-intercom',x+1.56,1.36,-6.024,.14,.30,.045,bronze);
        label('NORTH ENTRY',x,2.94,-6.86,1.3,.14,true);
        label('RESIDENTS',x+.69,1.75,-5.746,.78,.13,true);
      }
      // Plinth stops at doors rather than forming a raised doorstep across them.
      for(const [aa,bb]of [[left,x-half],[x+half,right]])box('rear-base',(aa+bb)/2,.16,-5.90,bb-aa-.012,.22,.22,dark);
    }else{
      const openings=[x-3.15,x+3.15];
      let from=left;
      for(const wx of openings){
        rearWall(from,wx-1.37,.29,3.18);
        rearWall(wx-1.37,wx+1.37,.29,1.62);
        rearWall(wx-1.37,wx+1.37,2.61,3.18);
        for(const xx of [wx-1.337,wx,wx+1.337])box('rear-high-window-frame',xx,2.115,-5.835,.045,.945,.09,bronze);
        for(const yy of [1.642,2.588])box('rear-high-window-edge',wx,yy,-5.835,2.72,.045,.09,bronze);
        for(const sign of [-1,1])box('rear-privacy-pane',wx+sign*.6685,2.115,-5.819,1.292,.90,.014,privacy);
        box('rear-window-sill',wx,1.589,-5.925,2.83,.038,.22,dark);
        from=wx+1.37;
      }
      rearWall(from,right,.29,3.18);
      box('rear-base',x,.16,-5.90,12.36,.22,.22,dark);
    }
  }
  // A continuous, level pedestrian strip distinguishes the rear access from asphalt.
  for(let col=0;col<62;col++)for(let row=0;row<3;row++)box('rear-paving',-30.5+col,.022,-6.43-row*.61,.986,.025,.595,paving);
  box('rear-path-edge',0,.022,-8.02,61.98,.025,.12,dark);
  for(const x of [-19,-6.2,6.2,19])planter(x,-6.39,2.5,.50);
  // A discreet stair direction marker points to the existing eastern external stair.
  label('←  RESIDENT STAIRS',28.7,1.50,-5.995,1.54,.17,true);
`+s.slice(b);s=s.replace('group.userData.podiumVersion=1','group.userData.podiumVersion=2');fs.writeFileSync(p,s);
