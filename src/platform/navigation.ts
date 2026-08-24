import { useCallback, useEffect, useRef, useState } from 'react';

export type MobileRoute = 'library' | 'discover' | 'collections' | 'stats' | 'settings' | 'form';
export type AndroidBackAction = 'history' | 'consumed' | 'exit';

export function androidBackActionForRoute(route: MobileRoute): AndroidBackAction {
  return route === 'library' ? 'exit' : 'history';
}

export function routeFromHash(hash: string | null | undefined): MobileRoute {
  const value = (hash ?? '').replace(/^#/, '');
  return ['library', 'discover', 'collections', 'stats', 'settings', 'form'].includes(value)
    ? value as MobileRoute
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
  androidShell?: 'root' | 'tab' | 'form';
  returnRoute?: MobileRoute;
}

function routeFromLocation(): MobileRoute {
  return routeFromHash(typeof window === 'undefined' ? '' : window.location.hash);
}

function stateFromHistory(): MobileHistoryState {
  return (typeof window === 'undefined' ? null : window.history.state) as MobileHistoryState | null ?? {};
}

function ensureRootHistory(): MobileRoute {
  if (typeof window === 'undefined') return 'library';
  const state = stateFromHistory();
  if (state.androidShell === 'root' || state.androidShell === 'tab' || state.androidShell === 'form') {
    return routeFromLocation();
  }
  window.history.replaceState({ mobileRoute: 'library', androidShell: 'root' }, '', '#library');
  return 'library';
}

export interface MobileNavigation {
  route: MobileRoute;
  navigate: (route: MobileRoute) => void;
  back: () => void;
  handleBack: () => AndroidBackAction;
}

export function useMobileNavigation(): MobileNavigation {
  const [route, setRoute] = useState<MobileRoute>(ensureRootHistory);
  const routeRef = useRef(route);

  useEffect(() => {
    const onPopState = () => {
      const next = routeFromLocation();
      routeRef.current = next;
      setRoute(next);
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
      window.history.pushState({ ...currentState, mobileRoute: 'form', androidShell: 'form', returnRoute: current }, '', '#form');
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
  }, []);

  const handleBack = useCallback((): AndroidBackAction => {
    return androidBackActionForRoute(routeRef.current);
  }, []);

  const back = useCallback(() => {
    if (handleBack() === 'history') window.history.back();
  }, [handleBack]);

  return { route, navigate, back, handleBack };
}
