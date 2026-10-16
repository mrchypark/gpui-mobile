package dev.gpui.mobile;
public final class GpuiContactsContractTest {
    public static void main(String[] args) {
        if (!"김\\%\\_\\\\".equals(GpuiContacts.escapeSearch("김%_\\"))) throw new AssertionError("literal search escaped incorrectly");
        if (!"Alice".equals(GpuiContacts.escapeSearch("Alice"))) throw new AssertionError();
        try { GpuiContacts.escapeSearch(null); throw new AssertionError(); }
        catch (IllegalArgumentException expected) { }
        System.out.println("PASS contacts literal search policy");
    }
}
