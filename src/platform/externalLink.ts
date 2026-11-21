/**
 * Safely open an external URL in the default browser or registered app
 * using Tauri's shell plugin, falling back to window.open.
 */
export function openExternalUrl(url: string): void {
  try {
    import('@tauri-apps/plugin-shell')
      .then(({ open }) => {
        void open(url).catch(() => {
          window.open(url, '_blank');
        });
      })
      .catch(() => {
        window.open(url, '_blank');
      });
  } catch {
    window.open(url, '_blank');
  }
}
