// Sidebar state store using Svelte 5 runes
import { SvelteSet } from 'svelte/reactivity';

type GroupId =
  'library' | 'media' | 'livetv' | 'live' | 'watching' | 'activity' | 'settings' | 'dev';

let collapsed = $state(false);
const defaultGroups: GroupId[] = [
  'library',
  'media',
  'livetv',
  'live',
  'watching',
  'activity',
  'settings'
];
let expandedGroups = new SvelteSet<GroupId>(defaultGroups);

// Initialize from localStorage
if (typeof localStorage !== 'undefined') {
  const savedCollapsed = localStorage.getItem('kobbler-sidebar-collapsed');
  if (savedCollapsed === 'true') {
    collapsed = true;
  }

  const savedGroups = localStorage.getItem('kobbler-sidebar-groups');
  if (savedGroups) {
    try {
      const parsed = JSON.parse(savedGroups) as GroupId[];
      expandedGroups.clear();
      for (const groupId of parsed) {
        expandedGroups.add(groupId);
      }
    } catch {
      // Use defaults
    }
  }
}

export function getSidebarState() {
  return {
    get collapsed() {
      return collapsed;
    },
    get expandedGroups() {
      return expandedGroups;
    }
  };
}

export function setSidebarCollapsed(value: boolean) {
  collapsed = value;
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('kobbler-sidebar-collapsed', String(value));
  }
}

export function toggleSidebarCollapsed() {
  setSidebarCollapsed(!collapsed);
}

export function toggleGroup(groupId: GroupId) {
  if (expandedGroups.has(groupId)) {
    expandedGroups.delete(groupId);
  } else {
    expandedGroups.add(groupId);
  }
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('kobbler-sidebar-groups', JSON.stringify(Array.from(expandedGroups)));
  }
}

export function isGroupExpanded(groupId: GroupId): boolean {
  return expandedGroups.has(groupId);
}
