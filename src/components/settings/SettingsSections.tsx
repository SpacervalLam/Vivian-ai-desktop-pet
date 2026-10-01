import React, { Children, isValidElement, type ReactNode } from 'react';

function flattenFragments(children: ReactNode, prefix = ''): ReactNode[] {
  return Children.toArray(children).flatMap((child) =>
    isValidElement<{ children?: ReactNode }>(child) && child.type === React.Fragment
      ? flattenFragments(child.props.children, `${prefix}/${child.key}`)
      : [isValidElement(child) ? React.cloneElement(child, { key: `${prefix}/${child.key}` }) : child],
  );
}

function headingText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (isValidElement<{ children?: ReactNode }>(node)) {
    // Heading actions and status messages do not belong in the contents list.
    if (node.type === 'button') return '';
    return Children.toArray(node.props.children).map(headingText).find(Boolean) ?? '';
  }
  return '';
}

/** Present existing top-level setting groups as cards without remounting their fields on edits. */
export default function SettingsSections({ children, contentsLabel }: { children: ReactNode; contentsLabel: string }) {
  const sections: { title: string; nodes: ReactNode[] }[] = [];
  for (const node of flattenFragments(children)) {
    const isHeading = isValidElement<{ style?: React.CSSProperties }>(node)
      && node.type === 'div' && !!node.props.style?.borderLeft;
    if (isHeading || !sections.length) sections.push({ title: isHeading ? headingText(node) : '', nodes: [] });
    sections[sections.length - 1].nodes.push(isHeading && isValidElement<{ style?: React.CSSProperties }>(node)
      ? React.cloneElement(node, { style: { ...node.props.style, marginTop: 0 }, role: 'heading', 'aria-level': 2 } as React.HTMLAttributes<HTMLDivElement>)
      : node);
  }
  return <>
    {sections.filter((section) => section.title).length > 1 && <nav className="settings-contents" aria-label={contentsLabel}>
      {sections.map((section, index) => section.title && <button type="button" key={index}
        onClick={() => document.getElementById(`settings-section-${index}`)?.scrollIntoView({ block: 'start' })}>
        {section.title}
      </button>)}
    </nav>}
    <div className="settings-sections">
      {sections.map((section, index) => <section className="settings-card" id={`settings-section-${index}`}
        aria-label={section.title || undefined} key={index}>{section.nodes}</section>)}
    </div>
  </>;
}
