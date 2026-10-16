#include <jni.h>
/* Test-only sink. Android View/Editable/BaseInputConnection remain real. */
static jclass recorder;
static jmethodID commit_method, composition_method, delete_method;
static jlong epoch(JNIEnv *env, jclass cls) { return 1; }
static jboolean logical(JNIEnv *env, jclass cls) { return JNI_TRUE; }
static void commit(JNIEnv *env, jclass cls, jlong session, jlong editor, jint action, jstring text) {
    (*env)->CallStaticVoidMethod(env, recorder, commit_method, session, editor, action, text);
}
static void compose(JNIEnv *env, jclass cls, jlong session, jlong editor, jstring text, jint start, jint end) {
    (*env)->CallStaticVoidMethod(env, recorder, composition_method, session, editor, text, start, end);
}
static jboolean delete_before(JNIEnv *env, jclass cls, jlong session, jlong editor, jint count, jboolean points) {
    return (*env)->CallStaticBooleanMethod(env, recorder, delete_method, session, editor, count, points);
}
JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *reserved) {
    JNIEnv *env;
    if ((*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
    jclass local = (*env)->FindClass(env, "dev/gpui/mobile/KeyboardRegression");
    if (!local) return JNI_ERR;
    recorder = (*env)->NewGlobalRef(env, local);
    commit_method = (*env)->GetStaticMethodID(env, local, "recordCommit", "(JJILjava/lang/String;)V");
    composition_method = (*env)->GetStaticMethodID(env, local, "recordComposition", "(JJLjava/lang/String;II)V");
    delete_method = (*env)->GetStaticMethodID(env, local, "recordDelete", "(JJIZ)Z");
    if (!recorder || !commit_method || !composition_method || !delete_method) return JNI_ERR;
    jclass keyboard = (*env)->FindClass(env, "dev/gpui/mobile/GpuiKeyboard");
    if (!keyboard) return JNI_ERR;
    JNINativeMethod methods[] = {
        {"editorEpoch", "()J", (void *)epoch}, {"hasLogicalOwner", "()Z", (void *)logical},
        {"committed", "(JJILjava/lang/String;)V", (void *)commit},
        {"composed", "(JJLjava/lang/String;II)V", (void *)compose},
        {"deleteBefore", "(JJIZ)Z", (void *)delete_before}
    };
    if ((*env)->RegisterNatives(env, keyboard, methods, 5) != JNI_OK) return JNI_ERR;
    return JNI_VERSION_1_6;
}
