# CIDR Range Scanning Examples

This file demonstrates how to use CIDR range scanning in proxy-scraper-checker.

## Basic CIDR Examples

# Single IP (equivalent to 192.168.1.100:8080)
192.168.1.100/32:8080

# Small subnet (4 IPs: .0, .1, .2, .3)  
192.168.1.0/30:3128

# Medium subnet (8 IPs: .0 through .7)
10.0.0.0/29:1080

# Larger subnet (16 IPs: .240 through .255)
172.16.1.240/28:8888

# Class C subnet (256 IPs: .0 through .255)
203.0.113.0/24:9090

## Mixed with Regular Entries

# You can mix CIDR ranges with regular IP:port entries:
192.168.1.0/30:8080
127.0.0.1:8888
10.0.0.0/31:3128
8.8.8.8:53

## Comments and Invalid Lines

# Lines starting with # are treated as comments and ignored
# Invalid CIDR ranges are preserved as-is for the regular parser
invalid-cidr-range:1234
not-an-ip:port

# Different protocols can use the same format
# Just put them in the appropriate protocol section in your config