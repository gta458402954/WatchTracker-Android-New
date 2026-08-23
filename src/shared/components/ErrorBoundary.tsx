import { Component, ErrorInfo, ReactNode } from 'react';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  public state: State = {
    hasError: false,
    error: null,
  };

  public static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error('[ErrorBoundary] Uncaught error:', error, errorInfo);
  }

  public render() {
    if (this.state.hasError) {
      return (
        <div className="min-h-screen bg-gray-50 flex items-center justify-center p-4">
          <div className="bg-white rounded-3xl shadow-xl p-8 max-w-md w-full text-center">
            <span className="text-6xl mb-6 block">🚧</span>
            <h1 className="text-2xl font-bold text-gray-900 mb-2">程序遇到了点小麻烦</h1>
            <p className="text-gray-500 mb-6">界面渲染出错了，你可以尝试重启程序。</p>
            <div className="bg-red-50 text-red-700 p-4 rounded-xl text-xs font-mono text-left overflow-auto max-h-40 mb-6">
              {this.state.error?.toString()}
            </div>
            <button
              onClick={() => window.location.reload()}
              className="w-full py-3 bg-indigo-600 text-white rounded-xl font-semibold hover:bg-indigo-700 transition-colors"
            >
              尝试刷新
            </button>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
