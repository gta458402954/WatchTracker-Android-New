import { useCallback, useEffect, useRef, useState } from 'react';

declare global { interface Window { __WATCHTRACKER_ANDROID_BACK_OVERLAY__?: () => boolean; } }

export type MobileRoute = 'library' | 'discover' | 'collections' | 'stats' | 'settings' | 'detail' | 'form';
export type MobileFormMode = 'new' | 'edit';
export type AndroidBackAction = 'history' | 'consumed' | 'exit';

export function androidBackActionForRoute(route: MobileRoute): AndroidBackAction {
  return route === 'library' ? 'exit' : 'history';
}

/** Contract consumed by the generated Android bridge after evaluateJavascript. */
export function androidBackResultForBridge(action: AndroidBackAction, canGoBack: boolean): 'finish' | 'history' | 'consumed' {
  if (action === 'consumed') return 'consumed';
  if (action === 'history' && canGoBack) return 'history';
  return 'finish';
}

export function routeFromHash(hash: string | null | undefined): MobileRoute {
  const value = (hash ?? '').replace(/^#/, '');
  const route = value.split('/')[0];
  return ['library', 'discover', 'collections', 'stats', 'settings', 'detail', 'form'].includes(route)
    ? route as MobileRoute
    : 'library';
}

export type NavigationHistoryMode = 'internal' | 'replace';

/** Tabs are represented by one replaceable tab entry. Forms are a real
 * history entry so Android's back dispatcher can close them before leaving.
 * The library is the shell root: Android Back exits directly from it instead
 * of traversing an unrelated browser entry underneath the tab UI. */
export function navigationMode(current: MobileRoute, next: MobileRoute): NavigationHistoryMode {
  return current === next ? 'replace' : 'internal';
}

interface MobileHistoryState {
  mobileRoute?: MobileRoute;
  androidShell?: 'root' | 'tab' | 'detail' | 'form';
  returnRoute?: MobileRoute;
}

function routeFromLocation(): MobileRoute {
  return routeFromHash(typeof window === 'undefined' ? '' : window.location.hash);
}

export function recordIdFromHash(hash: string | null | undefined): string | null {
  const value = (hash ?? '').replace(/^#/, '');
  if (value.startsWith('detail/')) return decodeURIComponent(value.slice('detail/'.length));
  if (value.startsWith('form/edit/')) return decodeURIComponent(value.slice('form/edit/'.length));
  return null;
}

function detailIdFromLocation(): string | null {
  return recordIdFromHash(typeof window === 'undefined' ? '' : window.location.hash);
}

function formModeFromLocation(): MobileFormMode {
  const value = typeof window === 'undefined' ? '' : window.location.hash.replace(/^#/, '');
  return value.startsWith('form/edit/') ? 'edit' : 'new';
}

function stateFromHistory(): MobileHistoryState {
  return (typeof window === 'undefined' ? null : window.history.state) as MobileHistoryState | null ?? {};
}

function ensureRootHistory(): MobileRoute {
  if (typeof window === 'undefined') return 'library';
  const state = stateFromHistory();
  if (state.androidShell === 'root' || state.androidShell === 'tab' || state.androidShell === 'detail' || state.androidShell === 'form') {
    return routeFromLocation();
  }
  const route = routeFromLocation();
  const hash = typeof window === 'undefined' ? '' : window.location.hash;
  const androidShell = route === 'library' ? 'root' : route === 'detail' ? 'detail' : route === 'form' ? 'form' : 'tab';
  window.history.replaceState({ mobileRoute: route, androidShell, detailId: detailIdFromLocation(), formMode: formModeFromLocation() }, '', hash || '#library');
  return route;
}

export interface MobileNavigation {
  route: MobileRoute;
  detailId: string | null;
  formMode: MobileFormMode;
  navigate: (route: MobileRoute) => void;
  navigateDetail: (id: string) => void;
  navigateForm: (mode: MobileFormMode, id?: string) => void;
  back: () => void;
  handleBack: () => AndroidBackAction;
}

export function useMobileNavigation(): MobileNavigation {
  const [route, setRoute] = useState<MobileRoute>(ensureRootHistory);
  const [detailId, setDetailId] = useState<string | null>(detailIdFromLocation);
  const [formMode, setFormMode] = useState<MobileFormMode>(formModeFromLocation);
  const routeRef = useRef(route);

  useEffect(() => {
    const onPopState = () => {
      const next = routeFromLocation();
      routeRef.current = next;
      setRoute(next);
      setDetailId(detailIdFromLocation());
      setFormMode(formModeFromLocation());
    };
    window.addEventListener('popstate', onPopState);
    window.addEventListener('hashchange', onPopState);
    return () => {
      window.removeEventListener('popstate', onPopState);
      window.removeEventListener('hashchange', onPopState);
    };
  }, []);

  const navigate = useCallback((next: MobileRoute) => {
    const current = routeRef.current;
    if (next === current) return;
    const currentState = stateFromHistory();
    if (next === 'form') {
      window.history.pushState({ ...currentState, mobileRoute: 'form', androidShell: 'form', returnRoute: current }, '', '#form/new');
    } else if (next === 'library') {
      window.history.replaceState({ ...currentState, mobileRoute: 'library', androidShell: 'root', returnRoute: undefined }, '', '#library');
    } else if (currentState.androidShell === 'root') {
      // Keep one replaceable tab entry, so multiple tab visits cannot leave a
      // stale placeholder underneath the library root.
      window.history.pushState({ ...currentState, mobileRoute: next, androidShell: 'tab' }, '', `#${next}`);
    } else {
      window.history.replaceState({ ...currentState, mobileRoute: next, androidShell: 'tab' }, '', `#${next}`);
    }
    routeRef.current = next;
    setRoute(next);
    setDetailId(null);
    setFormMode('new');
  }, []);

  const navigateDetail = useCallback((id: string) => {
    const currentState = stateFromHistory();
    window.history.pushState({ ...currentState, mobileRoute: 'detail', androidShell: 'detail', detailId: id }, '', `#detail/${encodeURIComponent(id)}`);
    routeRef.current = 'detail'; setRoute('detail'); setDetailId(id); setFormMode('new');
  }, []);

  const navigateForm = useCallback((mode: MobileFormMode, id?: string) => {
    const currentState = stateFromHistory();
    const hash = mode === 'edit' && id ? `#form/edit/${encodeURIComponent(id)}` : '#form/new';
    window.history.pushState({ ...currentState, mobileRoute: 'form', androidShell: 'form', formMode: mode, detailId: id ?? null }, '', hash);
    routeRef.current = 'form'; setRoute('form'); setDetailId(id ?? null); setFormMode(mode);
  }, []);

  const handleBack = useCallback((): AndroidBackAction => {
    if (typeof window !== 'undefined' && window.__WATCHTRACKER_ANDROID_BACK_OVERLAY__?.()) return 'consumed';
    return androidBackActionForRoute(routeRef.current);
  }, []);

  const back = useCallback(() => {
    if (handleBack() === 'history') window.history.back();
  }, [handleBack]);

  return { route, detailId, formMode, navigate, navigateDetail, navigateForm, back, handleBack };
}
