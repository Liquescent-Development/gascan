# The schema of engine/arca-pin.json, in one place.
#
# scripts/build-arca-engine.sh and scripts/sync-arca-proto.sh both read the pin
# and must agree on what a valid one is, or a pin one script accepts is one the
# other refuses. Until schema 2 that agreement was two copies of a jq program
# and a comment asking a maintainer to keep them in step; it is now a single
# file both scripts pass to `jq -e --from-file`, so the agreement cannot drift.
#
# Evaluates to true for a well-formed pin and false otherwise. It never reports
# WHICH clause failed: both callers exit 64 naming the pin file, and a schema
# that is also an error-message generator is a second thing to keep correct.
# `jq -e` makes false and null exit 1, so a caller needs nothing else.

# .tag is constrained to characters that cannot form a path. A tag name
# containing a slash is legal to git, and "tags/foo" as a pin would name two
# different objects to two different resolvers -- see the note on the
# verify-tag call in build-arca-engine.sh. The refs/tags/ qualification there is
# the real fix; this is the second lock on the same door, and it is the one that
# fails early and names the pin file.
def valid_tag: type == "string" and test("^[A-Za-z0-9._-]+$");

# .url is constrained to schemes git cannot turn into a command. `ext::` runs
# its argument as a shell command, so an unconstrained URL is arbitrary
# execution at clone time. file:// is admitted because the release contract's
# fixtures are local repositories.
def valid_git_url: type == "string" and length > 0 and test("^(https|file)://");

def valid_sha256: type == "string" and test("^[0-9a-f]{64}$");

# A byte length is a positive integer. `type == "number"` alone would admit 9.5
# and -1; floor and the bound refuse both. It exists so a truncated download is
# refused before its digest is even computed, which is the cheaper and clearer
# of the two refusals.
def valid_bytes: type == "number" and . == floor and . > 0;

# An artifact URL may differ in scheme and host from the git URL -- a release
# asset is not served by the git endpoint -- so it is validated separately, and
# the same two schemes are admitted for the same two reasons.
def valid_asset_url: type == "string" and length > 0 and test("^(https|file)://");

# .content records the identity that survives repackaging, and .kind names the
# procedure that recovers it. `tar czf` output is not reproducible -- entry
# order, mtimes and the gzip header vary run to run -- so an asset's own sha256
# dies the moment anyone repackages the same content. A pin carrying only the
# asset digest breaks on repackaging; one carrying only the inner digest cannot
# detect a truncated download. Both are required.
#
# The kind is enumerated rather than free text because an unrecognised kind is a
# content check the fetch cannot perform. Refusing it here fails closed at pin
# validation, where the message names the pin file, rather than at fetch time
# after ~83MB has been downloaded.
#
#   gzip-member  -- decompress the asset and hash the single member.
#   oci-manifest -- unpack the asset, read the OCI image index, and take the
#                   one manifest descriptor's digest and size.
def valid_content:
  type == "object" and
  (.kind | type == "string" and (. == "gzip-member" or . == "oci-manifest")) and
  (.bytes | valid_bytes) and
  (.sha256 | valid_sha256);

# .asset is a bare file name and not a path. It is what the asset is written as
# on disk and what names it in the release, so a "../" in it would place a
# fetched file outside the artifact directory.
def valid_artifact:
  type == "object" and
  (.asset | type == "string" and test("^[A-Za-z0-9._-]+$")) and
  (.url | valid_asset_url) and
  (.bytes | valid_bytes) and
  (.sha256 | valid_sha256) and
  (.content | valid_content);

# Both artifacts are required by name rather than accepted as a list. They land
# in different places and their content checks differ in kind, so a consumer has
# to know which is which; a positional list would make that an ordering
# convention instead of a name.
(.schema == 2) and
(.name | type == "string" and length > 0) and
(.url | valid_git_url) and
(.tag | valid_tag) and
(.revision | type == "string" and test("^[0-9a-f]{40}$")) and
(.artifacts | type == "object") and
(.artifacts.kernel | valid_artifact) and
(.artifacts.vminit | valid_artifact)
