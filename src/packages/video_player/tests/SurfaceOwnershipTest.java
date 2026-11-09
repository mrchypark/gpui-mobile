package dev.gpui.mobile;

/** Checks the production callback identity gate; native serialization needs device validation. */
public final class SurfaceOwnershipTest {
    public static void main(String[] args) {
        Object player = new Object(), oldSurface = new Object(), newSurface = new Object();
        check(true, player, player, oldSurface, oldSurface);
        // Replacement is registered: delayed old create/destroy must do nothing.
        check(false, player, player, newSurface, oldSurface);
        check(true, player, player, newSurface, newSurface);
        // Disposal or replacement of the player invalidates every old callback.
        check(false, null, player, newSurface, newSurface);
        check(false, new Object(), player, newSurface, newSurface);
        check(false, player, player, null, newSurface);
        System.out.println("SurfaceOwnershipTest PASS current/replaced/disposed identity gate");
    }
    private static void check(boolean expected, Object currentPlayer, Object player,
            Object currentSurface, Object surface) {
        if (GpuiVideoPlayer.ownsSurface(currentPlayer, player, currentSurface, surface) != expected) {
            throw new AssertionError("surface ownership mismatch");
        }
    }
}
