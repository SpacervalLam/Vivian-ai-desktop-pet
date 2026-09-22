import React from 'react';

interface Props {
  children: React.ReactNode;
  /** 页面切换时变化，用于重置错误状态（切到别的页就不再是崩溃态） */
  resetKey?: React.Key;
  fallback?: React.ReactNode;
}
interface State {
  error: Error | null;
}

/**
 * 页面级错误边界。
 *
 * 心智观察器是透明窗口：任何一个页面组件在「渲染期」抛错，整棵 React 树都会被
 * 错误边界（这里没有）一路冒泡到根、整窗卸载——透明窗口失去内容就表现为
 * 「打开了一个空窗口」。这里在页面内容外包一层边界，把单页崩溃关进兜底 UI，
 * 既不让整窗空白，也把真实错误打到 console 便于排查。
 *
 * 注意：错误边界只接住「渲染期 / 生命周期」的同步抛错；事件回调、effect 里
 * await 之后的异步抛错不在其捕获范围（那些本就不会卸载整树）。本边界足以覆盖
 * 「打开即空白」那一类崩溃——它们都发生在首屏渲染期。
 */
export default class PageErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('[MindInspector] 页面渲染崩溃:', error, info?.componentStack);
  }

  render(): React.ReactNode {
    if (this.state.error) {
      return (
        this.props.fallback ?? (
          <div className="mind-page-error" role="alert">
            <div className="mind-page-error-icon">!</div>
            <p className="mind-page-error-title">这一页加载失败了</p>
            <p className="mind-page-error-msg">{this.state.error.message}</p>
            <button
              type="button"
              className="mind-page-error-retry"
              onClick={() => this.setState({ error: null })}
            >
              重试
            </button>
          </div>
        )
      );
    }
    return this.props.children;
  }
}
