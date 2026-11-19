using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;

namespace doublyLinkedList
{
    public class ListNode
    {
        private Person p;
        public ListNode prev;
        public ListNode next;

        internal ListNode(Person p, ListNode prev, ListNode next)
        {
            this.P = p;
            this.Prev = prev;
            this.Next = next;
        }

        internal Person P
        {
            get
            {
                return this.p;
            }

            private set
            {
                if (value == null)
                {
                    throw new ArgumentNullException(nameof(this.P), $"This specific value {nameof(this.p)} cant be null");
                }

                this.p = value;
            }
        }

        public ListNode Prev
        {
            get
            {
                return this.prev;
            }

            private set
            {
                this.prev = value;
            }
        }

        public ListNode Next 
        {
            get
            {
                return this.next;
            }

            private set
            {
                this.next = value;
            }
        }
    }
}