/**
 * Creates a delayed loading state that only shows the loading indicator
 * after a specified delay. This prevents "loading flash" on fast loads.
 */
export function createDelayedLoading(initialLoading: boolean = false, delay: number = 150) {
  let loading = $state(initialLoading);
  let showLoading = $state(false);
  let timeoutId: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    if (loading) {
      timeoutId = setTimeout(() => {
        showLoading = true;
      }, delay);
    } else {
      if (timeoutId) clearTimeout(timeoutId);
      showLoading = false;
    }
    return () => {
      if (timeoutId) clearTimeout(timeoutId);
    };
  });

  return {
    get loading() {
      return loading;
    },
    set loading(v: boolean) {
      loading = v;
    },
    get showLoading() {
      return showLoading;
    }
  };
}
