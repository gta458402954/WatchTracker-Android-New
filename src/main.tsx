import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './app/App.tsx'
import ErrorBoundary from './shared/components/ErrorBoundary.tsx'
import './index.css'

// 立即挂载 React 应用
ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
)
