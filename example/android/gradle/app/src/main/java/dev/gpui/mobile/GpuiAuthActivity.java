package dev.gpui.mobile;

import android.os.Bundle;

import androidx.annotation.NonNull;
import androidx.biometric.BiometricPrompt;
import androidx.core.content.ContextCompat;
import androidx.fragment.app.FragmentActivity;

/**
 * Transparent helper Activity that displays a BiometricPrompt.
 *
 * <p>NativeActivity is not a FragmentActivity, so it cannot host a
 * BiometricPrompt directly. This lightweight Activity is launched by
 * {@link GpuiLocalAuth#authenticate} and immediately shows the prompt.
 * On completion (success, failure, or cancellation) it stores the result
 * in {@link GpuiLocalAuth#sResult}, counts down the latch, and finishes.</p>
 */
public class GpuiAuthActivity extends FragmentActivity {
    private GpuiLocalAuth.Session request;
    private BiometricPrompt prompt;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        request = GpuiLocalAuth.session(getIntent().getLongExtra("gpui.auth.request", -1));
        if (request == null) { finish(); return; }
        request.owner = new java.lang.ref.WeakReference<>(this);
        String reason = request.reason;
        if (reason == null || reason.isEmpty()) {
            reason = "Verify your identity";
        }

        BiometricPrompt.PromptInfo promptInfo = new BiometricPrompt.PromptInfo.Builder()
            .setTitle("Authentication Required")
            .setSubtitle(reason)
            .setAllowedAuthenticators(androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_WEAK)
            .setNegativeButtonText("Cancel")
            .build();

        BiometricPrompt biometricPrompt = new BiometricPrompt(this,
            ContextCompat.getMainExecutor(this),
            new BiometricPrompt.AuthenticationCallback() {
                @Override
                public void onAuthenticationSucceeded(@NonNull BiometricPrompt.AuthenticationResult result) {
                    super.onAuthenticationSucceeded(result);
                    request.complete(0);
                    finish();
                }

                @Override
                public void onAuthenticationFailed() {
                    super.onAuthenticationFailed();
                    // Called on each failed attempt; don't finish yet.
                    // The system will either allow retry or call onAuthenticationError.
                }

                @Override
                public void onAuthenticationError(int errorCode, @NonNull CharSequence errString) {
                    super.onAuthenticationError(errorCode, errString);
                    int result;
                    switch (errorCode) {
                        case BiometricPrompt.ERROR_USER_CANCELED:
                        case BiometricPrompt.ERROR_NEGATIVE_BUTTON:
                            result = 4; // cancelled
                            break;
                        case BiometricPrompt.ERROR_LOCKOUT:
                        case BiometricPrompt.ERROR_LOCKOUT_PERMANENT:
                            result = 6; // lockout
                            break;
                        case BiometricPrompt.ERROR_NO_BIOMETRICS:
                            result = 3; // not enrolled
                            break;
                        case BiometricPrompt.ERROR_HW_NOT_PRESENT:
                        case BiometricPrompt.ERROR_HW_UNAVAILABLE:
                            result = 2; // not available
                            break;
                        case BiometricPrompt.ERROR_NO_DEVICE_CREDENTIAL:
                            result = 5; // passcode not set
                            break;
                        default:
                            result = 7; // other
                            break;
                    }
                    request.complete(result);
                    finish();
                }
            });

        prompt = biometricPrompt;
        biometricPrompt.authenticate(promptInfo);
    }

    @Override
    public void onBackPressed() {
        if (request != null) request.complete(4);
        super.onBackPressed();
    }

    void cancelAndFinish() {
        if (prompt != null) prompt.cancelAuthentication();
        finish();
    }

    @Override protected void onDestroy() {
        if (request != null && !isChangingConfigurations()) request.complete(4);
        super.onDestroy();
    }
}
