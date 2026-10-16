import assert from 'node:assert/strict';
import { build } from 'esbuild';

const bundle = await build({
  entryPoints: ['src/components/ChatPageTransition.tsx'], bundle: true, write: false,
  platform: 'node', format: 'esm',
  plugins: [{ name: 'react-fixture', setup(b) {
    b.onResolve({ filter: /^react$|^react\/jsx-runtime$/ }, ({ path }) => ({ path, namespace: 'fixture' }));
    b.onLoad({ filter: /.*/, namespace: 'fixture' }, ({ path }) => ({ contents: path === 'react'
      ? 'class Component { constructor(props) { this.props = props; } } const createRef = () => ({ current: null }); export default { Component, createRef };'
      : 'export const jsx = (type, props) => ({ type, props }), jsxs = jsx;' }));
  } }],
});
const { default: Transition } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
let reduced = false;
const oldWindow = globalThis.window;
const globals = ['HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'HTMLCanvasElement'];
const saved = globals.map(key => globalThis[key]);
class Element {
  style = {}; attributes = new Map(); scrollTop = 0; scrollLeft = 0;
  children = []; animations = []; classes = [];
  classList = { add: value => this.classes.push(value) };
  setAttribute(key, value) { this.attributes.set(key, value); }
  removeAttribute(key) { this.attributes.delete(key); }
  querySelectorAll(selector) { return selector === '*' ? this.children : []; }
  cloneNode() {
    const copy = new this.constructor();
    copy.attributes = new Map(this.attributes);
    copy.children = this.children.map(child => child.cloneNode());
    return copy;
  }
  appendChild(child) { this.children.push(child); child.parent = this; }
  remove() { this.parent.children = this.parent.children.filter(child => child !== this); }
  animate(frames, options) {
    let resolve, reject;
    const finished = new Promise((yes, no) => { resolve = yes; reject = no; });
    const animation = { frames, options, finished, finish: resolve, cancelled: false,
      cancel() { this.cancelled = true; reject(new Error('cancelled')); } };
    this.animations.push(animation);
    return animation;
  }
}
try {
  globalThis.window = { matchMedia: () => ({ matches: reduced }) };
  for (const key of globals) globalThis[key] = class extends Element {};
  const page = new Transition({ pageKey: 'home', depth: 0 });
  const tree = page.render();
  const stage = new Element(), content = new Element();
  tree.props.ref.current = stage;
  tree.props.children.props.ref.current = content;
  const input = new HTMLInputElement(); input.value = 'unsaved draft'; input.checked = true;
  input.setAttribute('id', 'editor'); input.setAttribute('autofocus', '');
  const list = new Element(); list.scrollTop = 740; list.scrollLeft = 12;
  content.children = [input, list];
  const navigate = (key, depth) => {
    const previous = page.props;
    page.props = { pageKey: key, depth };
    const snapshot = page.getSnapshotBeforeUpdate(previous);
    page.componentDidUpdate(previous, null, snapshot);
    return snapshot;
  };
  assert.equal(navigate('home', 0), null, 'messages and other same-page updates do not animate');
  const snapshot = navigate('private:vivian', 1);
  assert.equal(content.animations.at(-1).frames[0].transform, 'translateX(100%)');
  assert.equal(snapshot.element.animations[0].frames[1].transform, 'translateX(-28%)');
  assert.equal(snapshot.element.attributes.has('inert'), true);
  assert.equal(snapshot.element.attributes.get('aria-hidden'), 'true');
  assert.equal(snapshot.element.children[0].value, 'unsaved draft');
  assert.equal(snapshot.element.children[0].checked, true);
  assert.equal(snapshot.element.children[0].attributes.has('id'), false);
  assert.equal(snapshot.element.children[0].attributes.has('autofocus'), false);
  assert.equal(snapshot.element.children[1].scrollTop, 740);
  assert.equal(snapshot.element.children[1].scrollLeft, 12);
  const firstAnimation = content.animations.at(-1);
  navigate('details:vivian:main', 2);
  assert.equal(firstAnimation.cancelled, true, 'rapid navigation cancels the old animation');
  assert.equal(stage.children.length, 1, 'only one outgoing snapshot survives');
  navigate('details:vivian:search', 3);
  const back = navigate('details:vivian:main', 2);
  assert.equal(content.animations.at(-1).frames[0].transform, 'translateX(-28%)');
  assert.equal(back.element.animations[0].frames[1].transform, 'translateX(100%)');
  assert.equal(back.element.style.zIndex, '2', 'departing child slides over its parent on return');
  content.animations.at(-1).finish(); back.element.animations[0].finish();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(stage.children.length, 0, 'completed animations remove snapshots');
  assert.equal(content.style.zIndex, '');
  navigate('assistant:vivian:notes', 1);
  page.componentWillUnmount();
  assert.equal(stage.children.length, 0, 'unmount cleans up interrupted animations');
  reduced = true;
  const count = content.animations.length;
  assert.equal(navigate('home', 0), null);
  assert.equal(content.animations.length, count, 'reduced motion switches immediately');
  reduced = false;
  content.animate = undefined;
  assert.equal(navigate('group', 1), null, 'older webviews without animation support switch immediately');
  console.log('Chat page transitions: push/pop, live-tree stability, drafts, scroll, rapid navigation, cleanup and reduced motion passed');
} finally {
  if (oldWindow === undefined) delete globalThis.window; else globalThis.window = oldWindow;
  globals.forEach((key, index) => { if (saved[index] === undefined) delete globalThis[key]; else globalThis[key] = saved[index]; });
}
