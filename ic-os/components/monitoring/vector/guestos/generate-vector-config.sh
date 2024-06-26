#!/bin/bash

# Substitute correct configuration parameters into vector.yml.

function usage() {
    cat <<EOF
Usage:
  generate-vector-config [-j vector.conf] \\
    -i vector.yaml.template \\
    -o vector.yaml

  Generate vector config from template file.

  -i infile: input vector.yaml.template file
  -j vector.conf: Optional, vector configuration description file
  -o outfile: output vector.yaml file
EOF
}

# Read the network config variables from file. The file must be of the form
# "key=value" for each line with a specific set of keys permissible (see
# code below).
#
# Arguments:
# - $1: Name of the file to be read.
function read_variables() {
    # Read limited set of keys. Be extra-careful quoting values as it could
    # otherwise lead to executing arbitrary shell code!
    while IFS="=" read -r key value; do
        case "$key" in
            "elasticsearch_hosts") elasticsearch_hosts="${value}" ;;
            "elasticsearch_tags") elasticsearch_tags="${value}" ;;
        esac
    done <"$1"
}

while getopts "i:j:k:o:" OPT; do
    case "${OPT}" in
        i)
            IN_FILE="${OPTARG}"
            ;;
        j)
            VECTOR_CONFIG_FILE="${OPTARG}"
            ;;
        o)
            OUT_FILE="${OPTARG}"
            ;;
        *)
            usage
            exit 1
            ;;
    esac
done

if [ "${IN_FILE}" == "" ] || [ "${OUT_FILE}" == "" ]; then
    usage
    exit 1
fi

if [ "${VECTOR_CONFIG_FILE}" != "" ] && [ -e "${VECTOR_CONFIG_FILE}" ]; then
    read_variables "${VECTOR_CONFIG_FILE}"
fi

ELASTICSEARCH_HOSTS="${elasticsearch_hosts}"
ELASTICSEARCH_TAGS="${elasticsearch_tags}"

if [ "${ELASTICSEARCH_HOSTS}" != "" ]; then
    # Covert string into comma separated array
    if [ "$(echo ${ELASTICSEARCH_HOSTS} | grep ':')" ]; then
        elasticsearch_hosts_array=$(for host in ${ELASTICSEARCH_HOSTS}; do echo -n "\"${host}\", "; done | sed -E "s@, \$@@g")
    else
        elasticsearch_hosts_array=$(for host in ${ELASTICSEARCH_HOSTS}; do echo -n "\"${host}:443\", "; done | sed -E "s@, \$@@g")
    fi
    sed -e "s@{{ elasticsearch_hosts }}@${elasticsearch_hosts_array}@" "${IN_FILE}" >"${OUT_FILE}"
fi

if [ "${ELASTICSEARCH_TAGS}" != "" ]; then
    # Covert string into comma separated array
    elasticsearch_tags_array=$(for tag in ${ELASTICSEARCH_TAGS}; do echo -n "\"${tag}\", "; done | sed -E "s@, \$@@g")
    sed -e "s@#{{ elasticsearch_tags }}@tags: [${elasticsearch_tags_array}]@" -i "${OUT_FILE}"
fi
