import assert from 'node:assert/strict';
import { build } from 'esbuild';

let slots = [], cursor = 0;
globalThis.__workflowJsonHooks = {
  useState(initial) {
    const index = cursor++;
    if (!(index in slots)) slots[index] = initial;
    return [slots[index], value => { slots[index] = typeof value === 'function' ? value(slots[index]) : value; }];
  },
};
const result = await build({ entryPoints: ['src/components/mind-inspector/pages/WorkflowJson.tsx'], bundle: true, write: false, format: 'esm', platform: 'node', plugins: [{
  name: 'json-interaction-hooks', setup(build) {
    build.onResolve({ filter: /^(react|react\/jsx-runtime|react-i18next)$/ }, args => ({ path: args.path, namespace: 'mock' }));
    build.onLoad({ filter: /.*/, namespace: 'mock' }, args => ({ contents: args.path === 'react'
      ? 'export const useState=(v)=>globalThis.__workflowJsonHooks.useState(v);'
      : args.path === 'react/jsx-runtime' ? 'export const jsx=(type,props,key)=>({type,props,key}),jsxs=jsx;'
      : 'export const useTranslation=()=>({t:(key,values)=>values ? `${key}:${JSON.stringify(values)}` : key});' }));
  },
}] });
const { default: WorkflowJson } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const find = (node, predicate) => !node || typeof node !== 'object' ? undefined
  : Array.isArray(node) ? node.map(child => find(child, predicate)).find(Boolean)
  : predicate(node) ? node : find(node.props?.children, predicate);
const all = (node, predicate) => !node || typeof node !== 'object' ? []
  : Array.isArray(node) ? node.flatMap(child => all(child, predicate))
  : [...(predicate(node) ? [node] : []), ...all(node.props?.children, predicate)];
const mount = (Component, props) => { slots = []; return () => { cursor = 0; return Component(props); }; };

const value = Array.from({ length: 125 }, (_, index) => ({ index, nested: [1, 2, 3] }));
let copied;
Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { clipboard: { writeText: async text => { copied = text; } } } });
let render = mount(WorkflowJson, { label: 'messages', value });
let tree = render();
await find(tree, node => node.type === 'button').props.onClick();
assert.deepEqual(JSON.parse(copied), value, 'copy includes the whole value, including unmounted JSON children');
tree = render();
assert.equal(find(tree, node => node.type === 'button').props.children, 'workflow.copied');
const root = find(tree, node => typeof node.type === 'function');

render = mount(root.type, root.props);
tree = render();
assert.equal(all(tree, node => typeof node.type === 'function').length, 0, 'collapsed objects do not mount descendants');
find(tree, node => node.type === 'details').props.onToggle({ currentTarget: { open: true } });
tree = render();
assert.equal(all(tree, node => typeof node.type === 'function').length, 50);
find(tree, node => node.type === 'button').props.onClick();
tree = render();
assert.equal(all(tree, node => typeof node.type === 'function').length, 100);
find(tree, node => node.type === 'button').props.onClick();
assert.equal(all(render(), node => typeof node.type === 'function').length, 125);

render = mount(root.type, { name: 'large', value: 'x'.repeat(9000) });
tree = render();
assert.ok(JSON.stringify(tree).length < 3000, 'large string is bounded until explicitly expanded');
find(tree, node => node.type === 'button').props.onClick();
assert.ok(JSON.stringify(render()).includes('x'.repeat(9000)));

navigator.clipboard.writeText = async () => { throw new Error('clipboard denied'); };
render = mount(WorkflowJson, { label: 'response', value: '<script>untrusted text</script>' });
await find(render(), node => node.type === 'button').props.onClick();
assert.ok(find(render(), node => node.props?.role === 'alert').props.children.includes('clipboard denied'));
console.log('Workflow JSON: lazy expansion, paging, complete copy, long text and clipboard failures passed.');
