#!/usr/bin/env ruby
#
# category: Git commands
# autocompletion: true
# config: up
# Write in bold
# opt:--handle-path:Whether we should handle paths found in the configuration
# opt:--handle-path:of the repository if any (yes/ask/no); When using \e[3mup\e[0m,
# opt:--handle-path:the \e[3mpath\e[0m configuration will be copied to the home
# opt:--handle-path:directory of the user to be loaded on every omni call. When
# opt:--handle-path:using \e[3mdown\e[0m, the \e[3mpath\e[0m configuration of the
# opt:--handle-path:repository will be removed from the home directory of the user
# opt:--handle-path:if it exists \e[90m(default: no)\e[0m
# help: Sets up or tear down a repository depending on its \e[3mup\e[0m configuration

require 'optparse'

require_relative '../lib/colorize'
require_relative '../lib/config'
require_relative '../lib/up/bundler_operation'
require_relative '../lib/up/custom_operation'
require_relative '../lib/up/homebrew_operation'
require_relative '../lib/up/ruby_operation'
require_relative '../lib/up/operation'
require_relative '../lib/utils'


options = {:handle_path => :no}
parser = OptionParser.new do |opts|
  opts.banner = "Usage: omni #{OmniEnv::OMNI_SUBCOMMAND} [options]"

  opts.on(
    "--handle-path [ACTION]", [:yes, :ask, :no],
    "Whether we should import/remove paths found in the repository if any (yes/ask/no)"
  ) do |handle_path|
    options[:handle_path] = handle_path || :ask
  end

  opts.on(
    "-h", "--help",
    "Prints this help"
  ) do
    `omni help #{OmniEnv::OMNI_SUBCOMMAND}`
    exit
  end

  opts.on(
    "--complete",
  ) do
    puts "--handle-path"
    puts "--help"
    puts "-h"
    exit
  end
end

begin
  parser.parse!
rescue OptionParser::InvalidOption, OptionParser::MissingArgument, OptionParser::InvalidArgument => e
  error(e.message)
end

error('too many arguments') if ARGV.size > 0
error("can only be run from a git repository") unless OmniEnv.in_git_repo?


def handle_path(proceed: false)
  return if OmniEnv::OMNI_SUBCOMMAND == 'down'

  Config.user_config_file(:readwrite) do |config|
    merged_path = {}
    [['append', :push], ['prepend', :unshift]].each do |key, func|
      merged_path[key] = config.dig('path', key).dup || []
      (Config.path_from_repo[key] || []).each do |path|
        merged_path[key].send(func, path) unless merged_path[key].include?(path)
      end
    end
    merged_path.select! { |_, value| !value.empty? }
    merged_path.transform_values! { |value| value.uniq }

    break if merged_path == config.dig('path')

    STDERR.puts "#{"omni:".light_cyan} #{"#{OmniEnv::OMNI_SUBCOMMAND}:".light_yellow} The current repository is declaring paths for omni commands."
    STDERR.puts "#{"omni:".light_cyan} #{"#{OmniEnv::OMNI_SUBCOMMAND}:".light_yellow} The following paths are going to be set in your configuration:"
    STDERR.puts "  #{"path:".green}"
    YAML.dump(merged_path).each_line do |line|
      line = line.chomp
      next if line == "---"
      STDERR.puts "    #{line.green}"
    end
    if config.dig('path', 'append') || config.dig('path', 'prepend')
      STDERR.puts "#{"omni:".light_cyan} #{"#{OmniEnv::OMNI_SUBCOMMAND}:".light_yellow} Previous configuration contained:"
      STDERR.puts "  #{"path:".red}"
      YAML.dump(config.dig('path')).each_line do |line|
        line = line.chomp
        next if line == "---"
        STDERR.puts "    #{line.red}"
      end
    end

    proceed = proceed || begin
      UserInterraction.confirm?("Do you want to continue?")
    rescue UserInterraction::StoppedByUserError, UserInterraction::NoMatchError
      false
    end

    if proceed
      STDERR.puts "#{"omni:".light_cyan} #{"#{OmniEnv::OMNI_SUBCOMMAND}:".light_yellow} Handled path."
      config['path'] = merged_path
      config
    else
      STDERR.puts "#{"omni:".light_cyan} #{"#{OmniEnv::OMNI_SUBCOMMAND}:".light_yellow} Skipped handling path."
      nil
    end
  end
end

def handle_up
  # Prepare all the commands that will need to be run, and check that the configuration is valid
  operations = Config.up.each_with_index.map do |operation, idx|
    operation = { operation => {} } if operation.is_a?(String)
    error("invalid #{'up'.yellow} configuration for operation #{idx.to_s.yellow}") \
      unless operation.is_a?(Hash) && operation.size == 1

    optype = operation.keys.first
    opconfig = operation[optype]

    cls = begin
      Object.const_get("#{optype.capitalize}Operation")
    rescue NameError
      error("invalid #{'up'.yellow} configuration for operation #{idx.to_s.yellow}: unknown operation #{optype.yellow}")
    end

    error("invalid #{'up'.yellow} configuration for operation #{idx.to_s.yellow}: invalid operation #{optype.yellow}") \
      unless cls < Operation

    cls.new(opconfig, index: idx)
  end

  # Run the commands from the git repository root
  Dir.chdir(OmniEnv.git_repo_root) do
    if OmniEnv::OMNI_SUBCOMMAND == 'up'
      # Run the operations in the provided order
      operations.each(&:up)
    else
      # In case of being called as `down`, this will also
      # run the operations in reverse order in case there
      # are dependencies between them
      operations.reverse.each(&:down)
    end
  end
end


should_handle_up = Config.respond_to?(:up) && Config.up
should_handle_path = [:yes, :ask].include?(options[:handle_path]) && Config.path_from_repo.any?

if should_handle_up || should_handle_path
  if should_handle_up
    error("invalid #{'up'.yellow} configuration, it should be a list") unless Config.up.is_a?(Array)
    handle_up
  end

  handle_path(proceed: options[:handle_path] == :yes) if should_handle_path
else
  STDERR.puts "#{"omni:".light_cyan} #{"#{OmniEnv::OMNI_SUBCOMMAND}:".light_yellow} No #{'up'.italic} configuration found, nothing to do."
end
