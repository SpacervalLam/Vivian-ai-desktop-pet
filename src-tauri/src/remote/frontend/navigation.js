/* global hideModal, goHome, openPage */
(() => {
  const drawer=document.getElementById('navigationDrawer');
  const nav=drawer.querySelector('nav');
  let previousFocus=null, suppressClickUntil=0, suppressedTarget=null, gesture=null;
  const isOpen=()=>drawer.classList.contains('open');
  window.openNavigation=()=>{
    if(isOpen())return;
    previousFocus=document.activeElement;
    previousFocus?.blur();
    drawer.inert=false;drawer.classList.add('open');drawer.setAttribute('aria-hidden','false');
    document.getElementById('app').inert=true;
    document.querySelectorAll('.modal-overlay').forEach(el=>el.inert=true);
    nav.querySelector('[aria-current="page"]')?.focus({preventScroll:true});
  };
  window.closeNavigation=()=>{
    const opened=isOpen();drawer.classList.remove('open');drawer.setAttribute('aria-hidden','true');
    document.getElementById('app').inert=false;
    document.querySelectorAll('.modal-overlay').forEach(el=>el.inert=false);
    if(opened && previousFocus?.isConnected)previousFocus.focus({preventScroll:true});
    drawer.inert=true;
  };
  window.navigateMobile=page=>{
    window.closeNavigation();
    document.querySelectorAll('.modal-overlay:not(.hidden)').forEach(el=>hideModal(el.id));
    if(page==='home')goHome();else openPage(null,page);
  };
  drawer.querySelector('.navigation-backdrop').onclick=window.closeNavigation;
  const start=(x,y,target)=>{gesture={x,y,target,dx:0,dy:0,locked:''};};
  const move=(x,y,event)=>{
    if(!gesture)return;
    gesture.dx=x-gesture.x;gesture.dy=y-gesture.y;
    const {dx,dy}=gesture;
    if(!gesture.locked && Math.max(Math.abs(dx),Math.abs(dy))>12) {
      gesture.locked=Math.abs(dx)>Math.abs(dy)*1.4 && ((!isOpen()&&dx>0)||(isOpen()&&dx<0))?'nav':'other';
    }
    if(gesture.locked==='nav' && event.cancelable)event.preventDefault();
  };
  const end=()=>{
    if(gesture?.locked==='nav' && Math.abs(gesture.dx)>=64 && Math.abs(gesture.dx)>Math.abs(gesture.dy)*1.4) {
      suppressClickUntil=performance.now()+350;
      suppressedTarget=gesture.target;
      if(gesture.dx>0)window.openNavigation();else window.closeNavigation();
    }
    gesture=null;
  };
  // Listen on the document so a right swipe can start anywhere, including subpages and forms.
  document.addEventListener('touchstart',event=>{
    if(event.touches.length!==1){gesture=null;return;}
    start(event.touches[0].clientX,event.touches[0].clientY,event.target);
  },{capture:true,passive:true});
  document.addEventListener('touchmove',event=>{
    if(event.touches.length!==1){gesture=null;return;}
    move(event.touches[0].clientX,event.touches[0].clientY,event);
  },{capture:true,passive:false});
  document.addEventListener('touchend',end,{capture:true});
  document.addEventListener('touchcancel',()=>{gesture=null;},{capture:true});
  document.addEventListener('mousedown',event=>{if(event.button===0)start(event.clientX,event.clientY,event.target);},true);
  document.addEventListener('mousemove',event=>{if(event.buttons&1)move(event.clientX,event.clientY,event);},true);
  document.addEventListener('mouseup',end,true);
  document.addEventListener('click',event=>{
    if(performance.now()<suppressClickUntil && !drawer.contains(event.target) && suppressedTarget &&
      (event.target===suppressedTarget || suppressedTarget.contains(event.target) || event.target.contains(suppressedTarget))) {
      event.preventDefault();event.stopImmediatePropagation();suppressClickUntil=0;
    }
  },true);
  document.addEventListener('keydown',event=>{
    if(!isOpen())return;
    if(event.key==='Escape'){event.preventDefault();event.stopImmediatePropagation();window.closeNavigation();}
    if(event.key==='Tab') {
      const buttons=[...nav.querySelectorAll('button:not([disabled])')];
      const first=buttons[0],last=buttons.at(-1);
      if(event.shiftKey&&document.activeElement===first){event.preventDefault();last.focus();}
      else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first.focus();}
    }
  },true);
  window.closeNavigation();
})();
