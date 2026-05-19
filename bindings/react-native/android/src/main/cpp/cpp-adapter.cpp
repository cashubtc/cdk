#include <jni.h>

#include "CashuDevKitOnLoad.hpp"

// JNI entrypoint. Nitrogen generates `initialize(vm)`, which registers every
// autolinked HybridObject (here, "OutputDataCreator") with the Nitro runtime.
JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void*) {
  return margelo::nitro::cashudevkit::initialize(vm);
}
