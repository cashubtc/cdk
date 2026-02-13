// Default storage backend using localStorage with "cdk:" prefix
let backend = {
    get(key) {
        try {
            return localStorage.getItem("cdk:" + key);
        } catch (_) {
            return null;
        }
    },
    set(key, value) {
        try {
            localStorage.setItem("cdk:" + key, value);
        } catch (_) {}
    },
    remove(key) {
        try {
            localStorage.removeItem("cdk:" + key);
        } catch (_) {}
    },
};

export function storageGet(key) {
    return backend.get(key);
}

export function storageSet(key, value) {
    backend.set(key, value);
}

export function storageRemove(key) {
    backend.remove(key);
}

// Swap the default backend at runtime.
// `newBackend` must implement { get(key)->string|null, set(key,value), remove(key) }
export function setStorageBackend(newBackend) {
    backend = newBackend;
}
