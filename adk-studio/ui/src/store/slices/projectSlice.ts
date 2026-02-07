import type { StateCreator } from 'zustand';
import type { Project, ProjectMeta, ProjectSettings } from '../../types/project';
import type { LayoutMode, LayoutDirection } from '../../types/layout';
import { api } from '../../api/client';
import { loadGlobalSettings } from '../../types/settings';

/**
 * Full store state type.
 * Defined here so the slice can access cross-slice state via get().
 * When slices are composed in the main store, this will be replaced
 * by the actual composed StudioState type.
 */
export interface StudioState {
  // Project slice state
  projects: ProjectMeta[];
  loadingProjects: boolean;
  currentProject: Project | null;

  // Cross-slice state accessed by project actions
  layoutMode: LayoutMode;
  layoutDirection: LayoutDirection;
  showDataFlowOverlay: boolean;
  debugMode: boolean;
  selectedNodeId: string | null;
  selectedActionNodeId: string | null;

  // Cross-slice actions accessed by project actions
  saveProject: () => Promise<void>;
}

export interface ProjectSlice {
  // State
  projects: ProjectMeta[];
  loadingProjects: boolean;
  currentProject: Project | null;

  // Actions
  fetchProjects: () => Promise<void>;
  createProject: (name: string, description?: string) => Promise<Project>;
  openProject: (id: string) => Promise<void>;
  saveProject: () => Promise<void>;
  closeProject: () => void;
  deleteProject: (id: string) => Promise<void>;
  updateProjectMeta: (name: string, description: string) => void;
  updateProjectSettings: (settings: Partial<ProjectSettings>) => void;
}

export const createProjectSlice: StateCreator<StudioState, [], [], ProjectSlice> = (set, get) => ({
  // State
  projects: [],
  loadingProjects: false,
  currentProject: null,

  // Actions
  fetchProjects: async () => {
    set({ loadingProjects: true });
    try {
      const projects = await api.projects.list();
      set({ projects });
    } finally {
      set({ loadingProjects: false });
    }
  },

  createProject: async (name, description) => {
    const project = await api.projects.create(name, description);
    set((s) => ({
      projects: [
        { id: project.id, name, description: description || '', updated_at: project.updated_at },
        ...s.projects,
      ],
    }));
    return project;
  },

  openProject: async (id) => {
    const project = await api.projects.get(id);
    const globalSettings = loadGlobalSettings();
    // Restore layout settings from project if available, otherwise use global defaults
    const layoutMode = project.settings?.layoutMode || globalSettings.layoutMode;
    const layoutDirection = project.settings?.layoutDirection || globalSettings.layoutDirection;
    const showDataFlowOverlay = project.settings?.showDataFlowOverlay ?? globalSettings.showDataFlowOverlay;
    const debugMode = project.settings?.debugMode ?? false;
    set({
      currentProject: project,
      selectedNodeId: null,
      layoutMode,
      layoutDirection,
      showDataFlowOverlay,
      debugMode,
    });
  },

  saveProject: async () => {
    const { currentProject, layoutMode, layoutDirection, showDataFlowOverlay, debugMode } = get();
    if (!currentProject) return;
    // Include layout settings and data flow overlay preference in project before saving
    const projectToSave = {
      ...currentProject,
      settings: {
        ...currentProject.settings,
        layoutMode,
        layoutDirection,
        showDataFlowOverlay,
        debugMode,
      },
    };
    await api.projects.update(currentProject.id, projectToSave);
  },

  closeProject: () => set({ currentProject: null, selectedNodeId: null, selectedActionNodeId: null }),

  deleteProject: async (id) => {
    await api.projects.delete(id);
    set((s) => ({ projects: s.projects.filter((p) => p.id !== id) }));
  },

  updateProjectMeta: (name, description) => {
    set((s) => {
      if (!s.currentProject) return s;
      return {
        currentProject: { ...s.currentProject, name, description },
        projects: s.projects.map((p) =>
          p.id === s.currentProject?.id ? { ...p, name, description } : p
        ),
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  updateProjectSettings: (settings) => {
    set((s) => {
      if (!s.currentProject) return s;
      const newSettings = { ...s.currentProject.settings, ...settings };
      // Also update local layout state if layout settings changed
      const updates: Partial<StudioState> = {
        currentProject: { ...s.currentProject, settings: newSettings },
      };
      if (settings.layoutMode !== undefined) updates.layoutMode = settings.layoutMode;
      if (settings.layoutDirection !== undefined) updates.layoutDirection = settings.layoutDirection;
      if (settings.showDataFlowOverlay !== undefined) updates.showDataFlowOverlay = settings.showDataFlowOverlay;
      return updates;
    });
    setTimeout(() => get().saveProject(), 0);
  },
});
