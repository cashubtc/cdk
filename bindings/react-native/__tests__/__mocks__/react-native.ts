// Mock React Native's TurboModuleRegistry for Jest tests
export const TurboModuleRegistry = {
  getEnforcing: (_name: string) => ({
    installRustCrate: () => true,
    cleanupRustCrate: () => true,
  }),
};
