# frozen_string_literal: true

# Invoked only with `brew ruby source_macos_formula.rb`, JSON request on stdin.
# Never resolve Formula["name"] or execute install/test: the recipe is the exact
# trusted .brew file retained by the package, not today's Homebrew formula.
require "json"
require "digest"
require "formulary"

# Loading a retained recipe must never install a tap or resolve another formula
# from today's API. Interpolations that need such lookups require explicit
# exact-provider support and fail closed here.
module RetainedFormulaLookupOnly
  def factory(*)
    raise "Source recipe attempted an unsupported formula lookup"
  end
end
Formulary.singleton_class.prepend(RetainedFormulaLookupOnly)
module NoSourceCollectionTapInstall
  def install(*, **)
    raise "Source recipe attempted an unsupported tap installation"
  end
end
Tap.prepend(NoSourceCollectionTapInstall)

request = JSON.parse($stdin.read)
recipe = Pathname(request.fetch("recipe"))
receipt = request.fetch("receipt")
raise "Expected a retained formula file" unless recipe.file? && recipe.basename.to_s == "#{request.fetch('name')}.rb"
source = recipe.read
# from_contents is the same loader Homebrew exposes for retained metadata, and
# accepts a staged recipe outside the live Cellar/tap without name resolution.
loader = Formulary.method(:from_contents)
options = { tap: Tap.fetch(receipt.fetch("source").fetch("tap")), flags: receipt.fetch("used_options") }
# Older runner Homebrew lacks this keyword. Inspect the API before calling it:
# rescuing ArgumentError could hide a recipe error and evaluate it a second time.
if loader.parameters.any? { |kind, name| %i[key keyreq].include?(kind) && name == :from_metadata }
  options[:from_metadata] = true
else
  require "formula"
  # Older Homebrew disables this historical update-automation annotation even
  # in retained recipes. Its supported string reason keeps autobump disabled;
  # leave every source/build DSL call and every other annotation untouched.
  module RetainedFormulaAutobumpReason
    def no_autobump!(because:)
      super(because: because == :requires_manual_review ? because.to_s : because)
    end
  end
  Formula.singleton_class.prepend(RetainedFormulaAutobumpReason)
end
formula = loader.call(request.fetch("name"), recipe, source, :stable, **options)
raise "Homebrew substituted another recipe" unless formula.path.realpath == recipe.realpath
raise "Formula version differs from installed receipt" unless formula.pkg_version.to_s == request.fetch("version")
raise "Formula name differs from provider" unless formula.name == request.fetch("name")

# Homebrew's resources list excludes resource declarations inside test methods
# and inactive platform branches. Audit the complete Ruby AST, evaluate only
# these resource DSL blocks, and include them too. Dynamic names/duplicates fail
# closed rather than letting a partial source list pass.
raise "This Homebrew Ruby needs RubyVM::AbstractSyntaxTree" unless defined?(RubyVM::AbstractSyntaxTree)
lines = source.lines
declarations = []
walk = lambda do |node|
  return unless node.is_a?(RubyVM::AbstractSyntaxTree::Node)

  if node.type == :ITER
    call = node.children.fetch(0)
    if call.type == :FCALL && call.children.fetch(0) == :resource
      args = call.children.fetch(1)
      name_node = args&.children&.fetch(0)
      raise "Unsupported dynamic resource declaration" unless name_node&.type == :STR

      name = name_node.children.fetch(0)
      raise "Ambiguous duplicate resource declaration: #{name}" if declarations.include?(name)

      declarations << name
      unless formula.resources.any? { |resource| resource.name == name }
        first, last = node.first_lineno - 1, node.last_lineno - 1
        excerpt = if first == last
          lines.fetch(first).byteslice(node.first_column...node.last_column)
        else
          lines.fetch(first).byteslice(node.first_column..) +
            lines[(first + 1)...last].join + lines.fetch(last).byteslice(0...node.last_column)
        end
        formula.instance_eval(excerpt, recipe.to_s, node.first_lineno)
      end
    elsif call.type == :CALL && call.children.fetch(1) == :resource
      raise "Unsupported receiver-qualified resource declaration"
    end
  end
  node.children.each { |child| walk.call(child) }
end
walk.call(RubyVM::AbstractSyntaxTree.parse(source))
raise "Resource declaration was not resolved" unless formula.resources.map(&:name).sort == declarations.sort

payloads = []
describe = lambda do |resource, id, role, name, extra_patches = []|
  strategy = resource.download_strategy.name
  specs = resource.specs
  item = { "id" => id, "role" => role, "name" => name, "url" => resource.url }
  if strategy == "GitDownloadStrategy"
    revision = specs[:revision]
    raise "Git resource lacks immutable revision" unless revision&.match?(/\A[0-9a-f]{40}(?:[0-9a-f]{24})?\z/)
    raise "Unsupported Git source options" unless (specs.keys - %i[using revision tag branch]).empty?

    item.merge!("kind" => "git", "revision" => revision)
  elsif %w[CurlDownloadStrategy CurlGitHubGitDownloadStrategy HomebrewCurlDownloadStrategy].include?(strategy)
    checksum = resource.checksum&.hexdigest
    raise "Source archive lacks SHA-256" unless checksum&.match?(/\A[0-9a-f]{64}\z/)
    # Request headers, POSTs, custom download strategies and transforms need an
    # explicit implementation. Do not silently fetch different content.
    allowed = %i[using verified]
    raise "Unsupported archive download options" unless (specs.keys - allowed).empty?

    item.merge!("kind" => "archive", "sha256" => checksum, "mirrors" => resource.mirrors)
  else
    raise "Unsupported source strategy: #{strategy}"
  end
  payloads << item
  (resource.patches + extra_patches).each_with_index do |patch, index|
    patch_id = "#{id}-patch-#{index}"
    details = { "id" => patch_id, "role" => "patch", "strip" => patch.strip.to_s }
    if defined?(LocalPatch) && patch.is_a?(LocalPatch)
      payloads << details.merge("kind" => "tap-file", "path" => patch.file.to_s)
    elsif patch.is_a?(ExternalPatch)
      offset = payloads.length
      describe.call(patch.resource, patch_id, "patch", patch.resource.name)
      payloads.fetch(offset).merge!("strip" => patch.strip.to_s, "apply" => patch.patch_files.map(&:to_s))
    elsif patch.is_a?(DATAPatch)
      patch.path = recipe
      payloads << details.merge("kind" => "inline", "contents" => patch.contents)
    elsif patch.is_a?(StringPatch)
      payloads << details.merge("kind" => "inline", "contents" => patch.contents)
    else
      raise "Unsupported patch: #{patch.class}"
    end
  end
end

describe.call(formula.stable.resource, "source", "source", formula.name, formula.stable.patches)
formula.resources.each_with_index do |resource, index|
  describe.call(resource, "resource-#{index}", "resource", resource.name)
end
plan = {
  "name" => formula.name, "version" => formula.pkg_version.to_s,
  "recipe_sha256" => Digest::SHA256.file(recipe).hexdigest,
  "resource_declarations" => declarations, "payloads" => payloads,
  "dependencies" => formula.deps.map { |dep| { "name" => dep.name, "tags" => dep.tags.map(&:to_s) } },
}
$stdout.write(JSON.generate(plan))
