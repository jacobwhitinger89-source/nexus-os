# NEXUS OS login environment.
export PS1='\u@\h:\w\$ '
# Administrators see the dashboard on interactive SSH logins.
# Set NEXUS_NO_DASHBOARD=1 (e.g. ssh -t host 'NEXUS_NO_DASHBOARD=1 sh -l') to skip.
if [ -n "$SSH_TTY" ] && [ -z "$NEXUS_NO_DASHBOARD" ] && [ -x /usr/bin/nexus ]; then
	case " $(id -Gn 2>/dev/null) " in
	*" wheel "*) nexus dashboard; echo "Type 'nexus help' for commands, 'exit' to log out." ;;
	esac
fi
