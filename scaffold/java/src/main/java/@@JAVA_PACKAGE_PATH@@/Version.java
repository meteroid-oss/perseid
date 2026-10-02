package @@JAVA_PACKAGE@@;

/** The version of the SDK. */
public final class Version {
    private Version() {}

    /** The version, sent in the {@code User-Agent} header. */
    public static final String VERSION = "@@VERSION@@"; // x-release-please-version
}
